//! Trusted-principal extraction for the human approval surface.
//!
//! Production requests use the assertion that Cloudflare Access places in the
//! Cf-Access-Jwt-Assertion header. The Worker verifies the assertion against
//! the configured Access issuer, audience, and rotating JWK set before it
//! exposes any principal claims to the D1 owner-identity check.

use serde::Deserialize;
use worker::js_sys::futures::JsFuture;
use worker::js_sys::{self, Object};
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::web_sys::{Crypto, CryptoKey, RsaHashedImportParams};
use worker::{Date, Env, Fetch, Request};

pub const ACCESS_JWT_ASSERTION_HEADER: &str = "Cf-Access-Jwt-Assertion";
pub const LOCAL_TEST_OWNER_HEADER: &str = "X-PRSync-Test-Owner";
pub const ACCESS_ISSUER_ENV: &str = "PRS_ACCESS_ISSUER";
pub const ACCESS_AUDIENCE_ENV: &str = "PRS_ACCESS_AUDIENCE";

#[cfg(feature = "local-test")]
const LOCAL_ENVIRONMENT: &str = "local";
const ACCESS_JWKS_SUFFIX: &str = "/cdn-cgi/access/certs";
const ACCESS_ALGORITHM: &str = "RS256";
const JWT_TYPE: &str = "JWT";
const MAX_JWT_SEGMENT_LENGTH: usize = 16 * 1024;

/// Identity claims that passed the trusted-principal input boundary.
///
/// The type has no public constructor. Production callers can create it only
/// by verifying the Access assertion. Local tests can create it only through
/// the explicit local-test feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedPrincipal {
    issuer: String,
    subject: String,
    email: Option<String>,
    display_name: Option<String>,
}

impl TrustedPrincipal {
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn email(&self) -> Option<&str> {
        self.email.as_deref()
    }

    pub fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    fn from_claims(claims: AccessClaims) -> Result<Self, PrincipalError> {
        let issuer = validate_claim(claims.iss, "issuer")?;
        if !issuer.starts_with("https://") {
            return Err(PrincipalError::Malformed("issuer must use https"));
        }
        let subject = validate_claim(claims.sub, "subject")?;
        Ok(Self {
            issuer,
            subject,
            email: optional_claim(claims.email, "email")?,
            display_name: optional_claim(claims.name, "name")?,
        })
    }

    #[cfg(any(test, feature = "local-test"))]
    pub fn for_local_test(
        issuer: impl Into<String>,
        subject: impl Into<String>,
        email: Option<String>,
    ) -> Result<Self, PrincipalError> {
        Self::from_claims(AccessClaims {
            iss: issuer.into(),
            sub: subject.into(),
            aud: AudienceClaim::One("local-test".to_owned()),
            exp: u64::MAX,
            nbf: None,
            email,
            name: None,
        })
    }
}

/// The only identity input accepted by the approval routes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalBoundary {
    allow_local_test_identity: bool,
    access: Option<AccessVerifierConfig>,
}

impl PrincipalBoundary {
    pub fn from_env(env: &Env) -> worker::Result<Self> {
        #[cfg(feature = "local-test")]
        let environment = env
            .var("PRS_ENVIRONMENT")
            .map(|value| value.to_string())
            .unwrap_or_default();

        #[cfg(feature = "local-test")]
        let allow_local_test_identity = environment == LOCAL_ENVIRONMENT;
        #[cfg(not(feature = "local-test"))]
        let allow_local_test_identity = false;

        if allow_local_test_identity {
            return Ok(Self {
                allow_local_test_identity,
                access: None,
            });
        }

        let issuer = env.var(ACCESS_ISSUER_ENV)?.to_string();
        let audience = env.var(ACCESS_AUDIENCE_ENV)?.to_string();
        let access = AccessVerifierConfig::new(issuer, audience)
            .map_err(|error| worker::Error::RustError(error.to_string()))?;
        Ok(Self {
            allow_local_test_identity,
            access: Some(access),
        })
    }

    /// Build the same trusted-principal value used by the local HTTP seam.
    /// This constructor is available only to local integration-test builds.
    #[cfg(feature = "local-test")]
    pub fn local_test_principal(value: &str) -> Result<TrustedPrincipal, PrincipalError> {
        parse_local_test_identity(value)
    }

    /// Extract a principal from a platform-authenticated request.
    ///
    /// In local Wrangler mode, the test header is accepted only when the
    /// Worker was built with local-test and the environment is explicitly
    /// local. Production always requires a verified Access assertion.
    pub async fn principal(&self, request: &Request) -> Result<TrustedPrincipal, PrincipalError> {
        if self.allow_local_test_identity {
            if let Some(value) = request
                .headers()
                .get(LOCAL_TEST_OWNER_HEADER)
                .map_err(|_| PrincipalError::Malformed("invalid test identity header"))?
            {
                return parse_local_test_identity(&value);
            }
        }

        let assertion = request
            .headers()
            .get(ACCESS_JWT_ASSERTION_HEADER)
            .map_err(|_| PrincipalError::Malformed("invalid Access assertion header"))?
            .ok_or(PrincipalError::MissingAssertion)?;
        let access = self.access.as_ref().ok_or(PrincipalError::Malformed(
            "Access verification is not configured",
        ))?;
        verify_access_assertion(&assertion, access).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrincipalError {
    MissingAssertion,
    Malformed(&'static str),
}

impl std::fmt::Display for PrincipalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingAssertion => formatter.write_str("trusted principal assertion is missing"),
            Self::Malformed(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for PrincipalError {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AccessVerifierConfig {
    issuer: String,
    audience: String,
    jwks_url: String,
}

impl AccessVerifierConfig {
    fn new(issuer: String, audience: String) -> Result<Self, PrincipalError> {
        let issuer = normalize_issuer(issuer)?;
        let audience = validate_claim(audience, "audience")?;
        Ok(Self {
            jwks_url: format!("{issuer}{ACCESS_JWKS_SUFFIX}"),
            issuer,
            audience,
        })
    }
}

#[derive(Debug, Deserialize)]
struct AccessHeader {
    alg: String,
    #[serde(default)]
    typ: Option<String>,
    #[serde(default)]
    kid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AccessClaims {
    iss: String,
    sub: String,
    aud: AudienceClaim,
    exp: u64,
    #[serde(default)]
    nbf: Option<u64>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AudienceClaim {
    One(String),
    Many(Vec<String>),
}

impl AudienceClaim {
    fn contains(&self, expected: &str) -> bool {
        match self {
            Self::One(value) => value == expected,
            Self::Many(values) => values.iter().any(|value| value == expected),
        }
    }
}

#[derive(Debug, Deserialize)]
struct JsonWebKeySet {
    keys: Vec<JsonWebKey>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct JsonWebKey {
    kid: String,
    kty: String,
    #[serde(default)]
    alg: Option<String>,
    #[serde(rename = "use", default)]
    key_use: Option<String>,
    n: String,
    e: String,
}

#[derive(Debug)]
struct ParsedAssertion {
    header_segment: String,
    claims_segment: String,
    signature_segment: String,
    header: AccessHeader,
    claims: AccessClaims,
}

impl ParsedAssertion {
    fn parse(assertion: &str) -> Result<Self, PrincipalError> {
        let mut parts = assertion.split('.');
        let header_segment = parts
            .next()
            .ok_or(PrincipalError::Malformed("invalid Access assertion"))?;
        let claims_segment = parts
            .next()
            .ok_or(PrincipalError::Malformed("invalid Access assertion"))?;
        let signature_segment = parts
            .next()
            .ok_or(PrincipalError::Malformed("invalid Access assertion"))?;
        if parts.next().is_some() || signature_segment.is_empty() {
            return Err(PrincipalError::Malformed("invalid Access assertion"));
        }

        let header: AccessHeader = decode_json(header_segment)?;
        if header.alg != ACCESS_ALGORITHM || header.typ.as_deref() != Some(JWT_TYPE) {
            return Err(PrincipalError::Malformed(
                "Access assertion must use a signed JWT",
            ));
        }
        let claims = decode_json(claims_segment)?;
        Ok(Self {
            header_segment: header_segment.to_owned(),
            claims_segment: claims_segment.to_owned(),
            signature_segment: signature_segment.to_owned(),
            header,
            claims,
        })
    }
}

async fn verify_access_assertion(
    assertion: &str,
    config: &AccessVerifierConfig,
) -> Result<TrustedPrincipal, PrincipalError> {
    let parsed = ParsedAssertion::parse(assertion)?;
    validate_access_claims(&parsed.claims, config, now_seconds())?;
    let key_id = parsed
        .header
        .kid
        .as_deref()
        .ok_or(PrincipalError::Malformed(
            "Access assertion has no signing key",
        ))?;
    let key_set = fetch_key_set(&config.jwks_url).await?;
    let key = select_signing_key(&key_set, key_id)?;
    let signing_input = format!("{}.{}", parsed.header_segment, parsed.claims_segment);
    let signature = base64url_decode(&parsed.signature_segment)?;
    if !verify_rsa_signature(key, signing_input.as_bytes(), &signature).await? {
        return Err(PrincipalError::Malformed(
            "Access assertion signature is invalid",
        ));
    }
    TrustedPrincipal::from_claims(parsed.claims)
}

fn validate_access_claims(
    claims: &AccessClaims,
    config: &AccessVerifierConfig,
    now: u64,
) -> Result<(), PrincipalError> {
    if claims.iss != config.issuer {
        return Err(PrincipalError::Malformed(
            "Access assertion issuer is invalid",
        ));
    }
    if !claims.aud.contains(&config.audience) {
        return Err(PrincipalError::Malformed(
            "Access assertion audience is invalid",
        ));
    }
    if now >= claims.exp {
        return Err(PrincipalError::Malformed("Access assertion has expired"));
    }
    if claims.nbf.is_some_and(|not_before| now < not_before) {
        return Err(PrincipalError::Malformed("Access assertion is not active"));
    }
    validate_claim(claims.iss.clone(), "issuer")?;
    validate_claim(claims.sub.clone(), "subject")?;
    optional_claim(claims.email.clone(), "email")?;
    optional_claim(claims.name.clone(), "name")?;
    Ok(())
}

async fn fetch_key_set(url: &str) -> Result<JsonWebKeySet, PrincipalError> {
    let url = worker::Url::parse(url)
        .map_err(|_| PrincipalError::Malformed("Access signing-key URL is invalid"))?;
    let mut response = Fetch::Url(url)
        .send()
        .await
        .map_err(|_| PrincipalError::Malformed("Access signing keys are unavailable"))?;
    if response.status_code() != 200 {
        return Err(PrincipalError::Malformed(
            "Access signing keys are unavailable",
        ));
    }
    response
        .json::<JsonWebKeySet>()
        .await
        .map_err(|_| PrincipalError::Malformed("Access signing keys are invalid"))
}

fn select_signing_key<'a>(
    key_set: &'a JsonWebKeySet,
    key_id: &str,
) -> Result<&'a JsonWebKey, PrincipalError> {
    key_set
        .keys
        .iter()
        .find(|key| {
            key.kid == key_id
                && key.kty == "RSA"
                && key
                    .alg
                    .as_deref()
                    .is_none_or(|algorithm| algorithm == ACCESS_ALGORITHM)
                && key
                    .key_use
                    .as_deref()
                    .is_none_or(|key_use| key_use == "sig")
        })
        .ok_or(PrincipalError::Malformed(
            "Access signing key is not trusted",
        ))
}

async fn verify_rsa_signature(
    jwk: &JsonWebKey,
    data: &[u8],
    signature: &[u8],
) -> Result<bool, PrincipalError> {
    let crypto = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("crypto"))
        .map_err(|_| PrincipalError::Malformed("Web Crypto is unavailable"))?
        .dyn_into::<Crypto>()
        .map_err(|_| PrincipalError::Malformed("Web Crypto is unavailable"))?;
    let subtle = crypto.subtle();
    let jwk_json = serde_json::to_string(jwk)
        .map_err(|_| PrincipalError::Malformed("Access signing key is invalid"))?;
    let jwk = js_sys::JSON::parse(&jwk_json)
        .map_err(|_| PrincipalError::Malformed("Access signing key is invalid"))?
        .dyn_into::<Object>()
        .map_err(|_| PrincipalError::Malformed("Access signing key is invalid"))?;
    let import_algorithm = RsaHashedImportParams::new_with_str("SHA-256");
    let usages = js_sys::Array::new();
    usages.push(&JsValue::from_str("verify"));
    let key = JsFuture::from(
        subtle
            .import_key_with_object(
                "jwk",
                &jwk,
                &import_algorithm,
                false,
                &JsValue::from(usages),
            )
            .map_err(|_| PrincipalError::Malformed("Access signing key is invalid"))?,
    )
    .await
    .map_err(|_| PrincipalError::Malformed("Access signing key is invalid"))?
    .dyn_into::<CryptoKey>()
    .map_err(|_| PrincipalError::Malformed("Access signing key is invalid"))?;
    let valid = JsFuture::from(
        subtle
            .verify_with_str_and_u8_array_and_u8_array("RSASSA-PKCS1-v1_5", &key, signature, data)
            .map_err(|_| PrincipalError::Malformed("Access assertion signature is invalid"))?,
    )
    .await
    .map_err(|_| PrincipalError::Malformed("Access assertion signature is invalid"))?
    .as_bool()
    .ok_or(PrincipalError::Malformed(
        "Access assertion signature is invalid",
    ))?;
    Ok(valid)
}

fn normalize_issuer(value: String) -> Result<String, PrincipalError> {
    let value = value.trim_end_matches('/').to_owned();
    let parsed = worker::Url::parse(&value)
        .map_err(|_| PrincipalError::Malformed("Access issuer is invalid"))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || parsed.port().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(PrincipalError::Malformed("Access issuer is invalid"));
    }
    validate_claim(value, "issuer")
}

#[cfg(any(feature = "local-test", test))]
fn parse_local_test_identity(value: &str) -> Result<TrustedPrincipal, PrincipalError> {
    let mut parts = value.split('|');
    let issuer = parts
        .next()
        .ok_or(PrincipalError::Malformed("invalid local test identity"))?;
    let subject = parts
        .next()
        .ok_or(PrincipalError::Malformed("invalid local test identity"))?;
    let email = parts.next().map(str::to_owned);
    if parts.next().is_some() || issuer.is_empty() || subject.is_empty() {
        return Err(PrincipalError::Malformed("invalid local test identity"));
    }
    TrustedPrincipal::for_local_test(issuer, subject, email)
}

#[cfg(not(any(feature = "local-test", test)))]
fn parse_local_test_identity(_value: &str) -> Result<TrustedPrincipal, PrincipalError> {
    Err(PrincipalError::Malformed(
        "local test identity support is not enabled",
    ))
}

fn decode_json<T: for<'de> Deserialize<'de>>(value: &str) -> Result<T, PrincipalError> {
    let bytes = base64url_decode(value)?;
    serde_json::from_slice(&bytes).map_err(|_| PrincipalError::Malformed("invalid JWT JSON"))
}

fn validate_claim(value: String, label: &'static str) -> Result<String, PrincipalError> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(PrincipalError::Malformed(match label {
            "issuer" => "invalid issuer claim",
            "subject" => "invalid subject claim",
            "audience" => "invalid audience claim",
            _ => "invalid principal claim",
        }));
    }
    Ok(value)
}

fn optional_claim(
    value: Option<String>,
    label: &'static str,
) -> Result<Option<String>, PrincipalError> {
    value.map(|value| validate_claim(value, label)).transpose()
}

fn base64url_decode(value: &str) -> Result<Vec<u8>, PrincipalError> {
    if value.is_empty() || value.len() > MAX_JWT_SEGMENT_LENGTH || value.contains('=') {
        return Err(PrincipalError::Malformed("invalid JWT segment"));
    }
    let mut output = Vec::with_capacity(value.len() * 3 / 4);
    let mut accumulator = 0u32;
    let mut bits = 0u8;
    for byte in value.bytes() {
        let six_bits = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return Err(PrincipalError::Malformed("invalid JWT segment")),
        } as u32;
        accumulator = (accumulator << 6) | six_bits;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
            accumulator &= (1 << bits) - 1;
        }
    }
    if bits >= 6 || (bits > 0 && accumulator != 0) {
        return Err(PrincipalError::Malformed("invalid JWT segment"));
    }
    Ok(output)
}

fn now_seconds() -> u64 {
    Date::now().as_millis() / 1_000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_base64url(value: &str) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let bytes = value.as_bytes();
        let mut output = String::new();
        let mut index = 0;
        while index < bytes.len() {
            let first = bytes[index] as u32;
            let second = bytes.get(index + 1).copied().unwrap_or_default() as u32;
            let third = bytes.get(index + 2).copied().unwrap_or_default() as u32;
            output.push(ALPHABET[(first >> 2) as usize] as char);
            output.push(ALPHABET[((first & 3) << 4 | second >> 4) as usize] as char);
            if index + 1 < bytes.len() {
                output.push(ALPHABET[((second & 15) << 2 | third >> 6) as usize] as char);
            }
            if index + 2 < bytes.len() {
                output.push(ALPHABET[(third & 63) as usize] as char);
            }
            index += 3;
        }
        output
    }

    fn assertion(claims: &str) -> String {
        format!(
            "{}.{}.signature",
            encode_base64url(r#"{"alg":"RS256","typ":"JWT","kid":"current"}"#),
            encode_base64url(claims)
        )
    }

    fn claims(issuer: &str, audience: &str, exp: u64) -> String {
        format!(
            r#"{{"iss":"{issuer}","sub":"owner-1","aud":["{audience}"],"exp":{exp},"email":"owner@example.com","name":"Owner"}}"#
        )
    }

    fn claims_with_nbf(issuer: &str, audience: &str, exp: u64, nbf: u64) -> String {
        format!(
            r#"{{"iss":"{issuer}","sub":"owner-1","aud":["{audience}"],"exp":{exp},"nbf":{nbf},"email":"owner@example.com","name":"Owner"}}"#
        )
    }

    fn config() -> AccessVerifierConfig {
        AccessVerifierConfig::new(
            "https://team.cloudflareaccess.com/".to_owned(),
            "application-audience".to_owned(),
        )
        .unwrap()
    }

    #[test]
    fn access_assertion_parses_claims_without_trusting_them() {
        let parsed = ParsedAssertion::parse(&assertion(&claims(
            "https://team.cloudflareaccess.com",
            "application-audience",
            2_000,
        )))
        .unwrap();
        assert_eq!(parsed.claims.iss, "https://team.cloudflareaccess.com");
        assert_eq!(parsed.claims.sub, "owner-1");
        assert_eq!(parsed.header.kid.as_deref(), Some("current"));
    }

    #[test]
    fn issuer_audience_and_validity_claims_are_required() {
        let config = config();
        let valid = ParsedAssertion::parse(&assertion(&claims(
            "https://team.cloudflareaccess.com",
            "application-audience",
            2_000,
        )))
        .unwrap();
        assert!(validate_access_claims(&valid.claims, &config, 1_000).is_ok());

        let wrong_issuer = ParsedAssertion::parse(&assertion(&claims(
            "https://other.cloudflareaccess.com",
            "application-audience",
            2_000,
        )))
        .unwrap();
        assert!(validate_access_claims(&wrong_issuer.claims, &config, 1_000).is_err());

        let wrong_audience = ParsedAssertion::parse(&assertion(&claims(
            "https://team.cloudflareaccess.com",
            "other-application",
            2_000,
        )))
        .unwrap();
        assert!(validate_access_claims(&wrong_audience.claims, &config, 1_000).is_err());

        let expired = ParsedAssertion::parse(&assertion(&claims(
            "https://team.cloudflareaccess.com",
            "application-audience",
            1_000,
        )))
        .unwrap();
        assert!(validate_access_claims(&expired.claims, &config, 1_000).is_err());

        let not_active = ParsedAssertion::parse(&assertion(&claims_with_nbf(
            "https://team.cloudflareaccess.com",
            "application-audience",
            2_000,
            1_001,
        )))
        .unwrap();
        assert!(validate_access_claims(&not_active.claims, &config, 1_000).is_err());
    }

    #[test]
    fn key_selection_supports_current_and_rotated_access_keys() {
        let key_set = JsonWebKeySet {
            keys: vec![
                JsonWebKey {
                    kid: "previous".to_owned(),
                    kty: "RSA".to_owned(),
                    alg: Some("RS256".to_owned()),
                    key_use: Some("sig".to_owned()),
                    n: "previous-modulus".to_owned(),
                    e: "AQAB".to_owned(),
                },
                JsonWebKey {
                    kid: "current".to_owned(),
                    kty: "RSA".to_owned(),
                    alg: Some("RS256".to_owned()),
                    key_use: Some("sig".to_owned()),
                    n: "current-modulus".to_owned(),
                    e: "AQAB".to_owned(),
                },
            ],
        };
        assert_eq!(
            select_signing_key(&key_set, "previous").unwrap().kid,
            "previous"
        );
        assert_eq!(
            select_signing_key(&key_set, "current").unwrap().kid,
            "current"
        );
        assert!(select_signing_key(&key_set, "unknown").is_err());
    }

    #[test]
    fn malformed_or_unsigned_assertions_are_rejected() {
        assert!(matches!(
            ParsedAssertion::parse("not-a-jwt"),
            Err(PrincipalError::Malformed(_))
        ));
        let unsigned = format!(
            "{}.{}.signature",
            encode_base64url(r#"{"alg":"none","typ":"JWT"}"#),
            encode_base64url(&claims(
                "https://team.cloudflareaccess.com",
                "application-audience",
                2_000,
            ))
        );
        assert!(matches!(
            ParsedAssertion::parse(&unsigned),
            Err(PrincipalError::Malformed(_))
        ));
    }

    #[test]
    fn local_test_principal_uses_the_same_claim_shape() {
        let principal = parse_local_test_identity(
            "https://team.cloudflareaccess.com|owner-1|owner@example.com",
        )
        .unwrap();
        assert_eq!(principal.subject(), "owner-1");
        assert_eq!(principal.email(), Some("owner@example.com"));
    }
}
