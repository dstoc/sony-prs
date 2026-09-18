//! Trusted-principal extraction for the human approval surface.
//!
//! Production requests use the assertion that Cloudflare Access places in the
//! `Cf-Access-Jwt-Assertion` header. The Worker does not accept an identity
//! from a query parameter, form field, cookie, or ordinary browser header.
//! Cloudflare Access must validate the assertion before it reaches this
//! Worker. This module validates the assertion shape and exposes only its
//! principal claims to the D1 owner-identity check.

use serde::Deserialize;
use worker::{Env, Request};

pub const ACCESS_JWT_ASSERTION_HEADER: &str = "Cf-Access-Jwt-Assertion";
pub const LOCAL_TEST_OWNER_HEADER: &str = "X-PRSync-Test-Owner";

const LOCAL_ENVIRONMENT: &str = "local";

/// Identity claims that passed the trusted-principal input boundary.
///
/// The type has no public constructor. Production callers can create it only
/// by parsing the Access assertion. Local tests can create it only through the
/// explicit `local-test` feature.
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
            email,
            name: None,
        })
    }
}

/// The only identity input accepted by the approval routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrincipalBoundary {
    allow_local_test_identity: bool,
}

impl PrincipalBoundary {
    pub fn from_env(env: &Env) -> worker::Result<Self> {
        let environment = env
            .var("PRS_ENVIRONMENT")
            .map(|value| value.to_string())
            .unwrap_or_default();
        Ok(Self {
            allow_local_test_identity: environment == LOCAL_ENVIRONMENT,
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
    /// Worker environment is explicitly `local`. Production always requires
    /// the Access assertion header.
    pub fn principal(&self, request: &Request) -> Result<TrustedPrincipal, PrincipalError> {
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
        parse_access_assertion(&assertion)
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

#[derive(Debug, Deserialize)]
struct AccessHeader {
    alg: String,
    #[serde(default)]
    typ: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AccessClaims {
    iss: String,
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

fn parse_access_assertion(assertion: &str) -> Result<TrustedPrincipal, PrincipalError> {
    let mut parts = assertion.split('.');
    let header = parts
        .next()
        .ok_or(PrincipalError::Malformed("invalid Access assertion"))?;
    let claims = parts
        .next()
        .ok_or(PrincipalError::Malformed("invalid Access assertion"))?;
    let signature = parts
        .next()
        .ok_or(PrincipalError::Malformed("invalid Access assertion"))?;
    if parts.next().is_some() || signature.is_empty() {
        return Err(PrincipalError::Malformed("invalid Access assertion"));
    }

    let header: AccessHeader = decode_json(header)?;
    if header.alg != "RS256" || header.typ.as_deref() != Some("JWT") {
        return Err(PrincipalError::Malformed(
            "Access assertion must use a signed JWT",
        ));
    }
    TrustedPrincipal::from_claims(decode_json(claims)?)
}

#[cfg(any(test, feature = "local-test"))]
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

#[cfg(not(any(test, feature = "local-test")))]
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
    if value.is_empty() || value.len() > 16 * 1024 || value.contains('=') {
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
            encode_base64url(r#"{"alg":"RS256","typ":"JWT"}"#),
            encode_base64url(claims)
        )
    }

    #[test]
    fn access_assertion_produces_only_claimed_principal() {
        let principal = parse_access_assertion(&assertion(
            r#"{"iss":"https://team.cloudflareaccess.com","sub":"owner-1","email":"owner@example.com","name":"Owner"}"#,
        ))
        .unwrap();
        assert_eq!(principal.issuer(), "https://team.cloudflareaccess.com");
        assert_eq!(principal.subject(), "owner-1");
        assert_eq!(principal.email(), Some("owner@example.com"));
        assert_eq!(principal.display_name(), Some("Owner"));
    }

    #[test]
    fn malformed_or_unsigned_assertions_are_rejected() {
        assert!(matches!(
            parse_access_assertion("not-a-jwt"),
            Err(PrincipalError::Malformed(_))
        ));
        let unsigned = format!(
            "{}.{}.signature",
            encode_base64url(r#"{"alg":"none","typ":"JWT"}"#),
            encode_base64url(r#"{"iss":"https://team.cloudflareaccess.com","sub":"owner-1"}"#)
        );
        assert!(matches!(
            parse_access_assertion(&unsigned),
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
