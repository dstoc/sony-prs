//! Versioned wire types shared by the PRSync sender, Worker, and reader.
//!
//! This crate describes protocol data only. It does not create or store
//! credentials, access Cloudflare services, access a filesystem, or depend on
//! the reader UI and Markdown implementation.

use core::fmt;
use serde::{de, Deserialize, Deserializer, Serialize};

/// The protocol version implemented by this crate.
pub const CURRENT_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::new(1, 0);

/// The first supported uncompressed bundle manifest format.
pub const CURRENT_BUNDLE_FORMAT_VERSION: u16 = 1;

/// The maximum encoded bundle size specified by the PRSync protocol.
pub const MAX_BUNDLE_SIZE: u64 = 16 * 1024 * 1024;

/// A major version can change the wire contract. A minor version adds fields
/// or values that an older compatible reader does not need to understand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl ProtocolVersion {
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    pub const fn is_compatible_with(self, supported: Self) -> bool {
        self.major == supported.major && self.minor <= supported.minor
    }

    pub fn require_compatible(self, supported: Self) -> Result<(), VersionError> {
        if self.is_compatible_with(supported) {
            Ok(())
        } else {
            Err(VersionError {
                received: self,
                supported,
            })
        }
    }
}

impl Default for ProtocolVersion {
    fn default() -> Self {
        CURRENT_PROTOCOL_VERSION
    }
}

/// An incoming wire version is not understood by the selected implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionError {
    pub received: ProtocolVersion,
    pub supported: ProtocolVersion,
}

impl fmt::Display for VersionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "protocol version {}.{} is incompatible with supported version {}.{}",
            self.received.major, self.received.minor, self.supported.major, self.supported.minor
        )
    }
}

impl std::error::Error for VersionError {}

/// A monotonically changing inbox revision. Revision zero represents an
/// empty or never-published inbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InboxRevision(u64);

impl InboxRevision {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

/// An opaque HTTP entity-tag value without the HTTP header's surrounding
/// quotes. The Worker can add the header syntax at its transport boundary.
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct EntityTag(String);

impl EntityTag {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        validate_non_empty_ascii(&value, "entity tag")?;
        if value.contains('"') {
            return Err(ValidationError::new("entity tag must not contain quotes"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for EntityTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("EntityTag").field(&self.0).finish()
    }
}

impl<'de> Deserialize<'de> for EntityTag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

/// A path inside a bundle. It is a protocol string, not a host filesystem
/// path, and always uses `/` separators.
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct BundlePath(String);

impl BundlePath {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ValidationError::new("bundle path must not be empty"));
        }
        if value.starts_with('/') || value.contains('\\') || value.contains('\0') {
            return Err(ValidationError::new(
                "bundle path must be relative, use `/`, and contain no NUL",
            ));
        }
        if value
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
        {
            return Err(ValidationError::new(
                "bundle path contains an empty, `.` or `..` component",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BundlePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("BundlePath").field(&self.0).finish()
    }
}

impl<'de> Deserialize<'de> for BundlePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

macro_rules! opaque_identifier {
    ($name:ident, $label:literal) => {
        #[derive(Clone, PartialEq, Eq, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
                let value = value.into();
                validate_identifier(&value, $label)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.0)
                    .finish()
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
            }
        }
    };
}

opaque_identifier!(AuthorizationRequestId, "authorization request ID");
opaque_identifier!(CredentialId, "credential ID");
opaque_identifier!(SessionId, "session ID");
opaque_identifier!(RequestId, "request ID");

/// A human-assigned sender credential name.
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SenderCredentialName(String);

impl SenderCredentialName {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.is_empty() || value.len() > 64 {
            return Err(ValidationError::new(
                "sender credential name must contain 1 to 64 bytes",
            ));
        }
        if value.chars().any(char::is_control) {
            return Err(ValidationError::new(
                "sender credential name must not contain control characters",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SenderCredentialName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SenderCredentialName")
            .field(&self.0)
            .finish()
    }
}

impl<'de> Deserialize<'de> for SenderCredentialName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

/// A Unix timestamp in seconds. The protocol does not require a time library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(u64);

impl Timestamp {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

/// A polling secret held by the client while it waits for human approval.
/// This crate does not generate, persist, or hash the secret.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PollingSecret(String);

impl PollingSecret {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        validate_secret(&value, "polling secret")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PollingSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PollingSecret(REDACTED)")
    }
}

impl<'de> Deserialize<'de> for PollingSecret {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

/// A service-issued bearer token. The service and client own its lifecycle;
/// this crate only carries it in a claim result.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct BearerToken(String);

impl BearerToken {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        validate_secret(&value, "bearer token")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BearerToken(REDACTED)")
    }
}

impl<'de> Deserialize<'de> for BearerToken {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

/// A manifest entry for one regular file in a bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    pub path: BundlePath,
    pub size: u64,
}

/// The generated manifest at the root of a PRSync bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub protocol_version: ProtocolVersion,
    pub bundle_format_version: u16,
    pub entry_point: BundlePath,
    pub files: Vec<ManifestFile>,
}

/// A conditional inbox request. An omitted condition asks for the current
/// state without conditional matching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxManifestRequest {
    pub protocol_version: ProtocolVersion,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub if_revision: Option<InboxRevision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub if_none_match: Option<EntityTag>,
}

/// The result of an inbox manifest request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxManifestResponse {
    pub protocol_version: ProtocolVersion,
    pub state: InboxManifestState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum InboxManifestState {
    Empty {
        revision: InboxRevision,
    },
    Current {
        revision: InboxRevision,
        etag: EntityTag,
        manifest: Manifest,
    },
    NotModified {
        revision: InboxRevision,
        etag: EntityTag,
    },
}

/// The two human-approval flows supported by the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationKind {
    Sender,
    Reader,
}

/// Public information for a pending authorization request. The polling
/// secret is deliberately not part of this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationRequest {
    pub protocol_version: ProtocolVersion,
    pub request_id: AuthorizationRequestId,
    pub kind: AuthorizationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_name: Option<SenderCredentialName>,
    pub approval_url: String,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
}

/// The client-side result of starting an authorization request. The client
/// keeps the secret private while the public request is shown to the human.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationStart {
    pub protocol_version: ProtocolVersion,
    pub request: AuthorizationRequest,
    pub polling_secret: PollingSecret,
}

/// A status poll that does not expose a claimed bearer credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationStatus {
    pub protocol_version: ProtocolVersion,
    pub request_id: AuthorizationRequestId,
    pub kind: AuthorizationKind,
    pub state: AuthorizationState,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationState {
    Pending,
    Approved,
    Denied,
    Expired,
    Claimed,
}

/// A polling-secret claim request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollingSecretClaimRequest {
    pub protocol_version: ProtocolVersion,
    pub request_id: AuthorizationRequestId,
    pub polling_secret: PollingSecret,
}

/// The result returned by a polling-secret claim attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollingSecretClaimResult {
    pub protocol_version: ProtocolVersion,
    pub outcome: PollingSecretClaimOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PollingSecretClaimOutcome {
    Pending { retry_after_seconds: u32 },
    Sender { credential: SenderCredentialGrant },
    Reader { session: ReaderSession },
    Denied,
    Expired,
    AlreadyClaimed,
}

/// Sender operations are represented by a dedicated capability enum and
/// scope, rather than sharing reader permissions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SenderScope {
    pub capabilities: Vec<SenderCapability>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SenderCapability {
    UploadBundle,
    ClearInbox,
    ManageCredentials,
}

/// Reader operations are represented by a separate capability enum and
/// session scope. A reader scope has no sender operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderSessionScope {
    pub capabilities: Vec<ReaderCapability>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderCapability {
    ReadManifest,
    DownloadBundle,
}

/// Metadata that can be returned by credential listing and sender claims. It
/// never contains the bearer token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SenderCredentialMetadata {
    pub credential_id: CredentialId,
    pub name: SenderCredentialName,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<Timestamp>,
    pub scope: SenderScope,
}

/// A newly claimed sender credential. The token is returned only by the
/// successful polling claim response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SenderCredentialGrant {
    pub bearer_token: BearerToken,
    pub metadata: SenderCredentialMetadata,
}

/// A boot-scoped reader session. The reader stores this token only in memory
/// and must start a fresh authorization flow after reboot, at `expires_at`, or
/// after the service rejects the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderSession {
    pub session_id: SessionId,
    pub bearer_token: BearerToken,
    pub issued_at: Timestamp,
    /// The server-side session deadline. Older services may omit this field;
    /// such a session is not valid until the client has reauthorized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    pub scope: ReaderSessionScope,
}

impl ReaderSession {
    /// Return whether the server-side session deadline still permits use.
    /// Missing expiry metadata fails closed so the client can reauthorize.
    pub const fn is_valid_at(&self, now: Timestamp) -> bool {
        match self.expires_at {
            Some(expires_at) => now.value() < expires_at.value(),
            None => false,
        }
    }
}

/// Machine-readable API error codes. The human message is informative and
/// must not be used for client branching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiErrorCode {
    InvalidRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    Gone,
    PayloadTooLarge,
    UnsupportedVersion,
    RateLimited,
    Internal,
}

/// A stable error envelope for every protocol API failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub protocol_version: ProtocolVersion,
    pub error: ApiErrorBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub code: ApiErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,
}

/// Errors returned by the constructors that protect protocol string fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    message: &'static str,
}

impl ValidationError {
    const fn new(message: &'static str) -> Self {
        Self { message }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for ValidationError {}

fn validate_identifier(value: &str, label: &'static str) -> Result<(), ValidationError> {
    if value.is_empty() || value.len() > 128 {
        return Err(ValidationError::new(
            "identifier must contain 1 to 128 bytes",
        ));
    }
    if value
        .bytes()
        .any(|byte| !(byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte)))
    {
        return Err(ValidationError::new(match label {
            "authorization request ID" => {
                "authorization request ID contains an unsupported character"
            }
            "credential ID" => "credential ID contains an unsupported character",
            "session ID" => "session ID contains an unsupported character",
            "request ID" => "request ID contains an unsupported character",
            _ => "identifier contains an unsupported character",
        }));
    }
    Ok(())
}

fn validate_non_empty_ascii(value: &str, label: &'static str) -> Result<(), ValidationError> {
    if value.is_empty() || value.len() > 128 || !value.is_ascii() {
        return Err(ValidationError::new(match label {
            "entity tag" => "entity tag must contain 1 to 128 ASCII bytes",
            _ => "value must contain 1 to 128 ASCII bytes",
        }));
    }
    if value
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte == b' ')
    {
        return Err(ValidationError::new(
            "value must not contain spaces or control characters",
        ));
    }
    Ok(())
}

fn validate_secret(value: &str, label: &'static str) -> Result<(), ValidationError> {
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_whitespace) {
        return Err(ValidationError::new(match label {
            "polling secret" => "polling secret must be non-empty and contain no whitespace",
            "bearer token" => "bearer token must be non-empty and contain no whitespace",
            _ => "secret must be non-empty and contain no whitespace",
        }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version() -> ProtocolVersion {
        CURRENT_PROTOCOL_VERSION
    }

    fn path(value: &str) -> BundlePath {
        BundlePath::new(value).unwrap()
    }

    fn request_id(value: &str) -> AuthorizationRequestId {
        AuthorizationRequestId::new(value).unwrap()
    }

    fn credential_id(value: &str) -> CredentialId {
        CredentialId::new(value).unwrap()
    }

    fn session_id(value: &str) -> SessionId {
        SessionId::new(value).unwrap()
    }

    fn sender_name(value: &str) -> SenderCredentialName {
        SenderCredentialName::new(value).unwrap()
    }

    #[test]
    fn manifest_json_is_stable() {
        let manifest = Manifest {
            protocol_version: version(),
            bundle_format_version: CURRENT_BUNDLE_FORMAT_VERSION,
            entry_point: path("index.md"),
            files: vec![ManifestFile {
                path: path("index.md"),
                size: 42,
            }],
        };

        assert_eq!(
            serde_json::to_string(&manifest).unwrap(),
            r#"{"protocol_version":{"major":1,"minor":0},"bundle_format_version":1,"entry_point":"index.md","files":[{"path":"index.md","size":42}]}"#
        );
        assert_eq!(
            serde_json::from_str::<Manifest>(
                r#"{"protocol_version":{"major":1,"minor":0},"bundle_format_version":1,"entry_point":"index.md","files":[{"path":"index.md","size":42}]}"#
            )
            .unwrap(),
            manifest
        );
    }

    #[test]
    fn manifest_ignores_optional_legacy_sha256_metadata() {
        let manifest: Manifest = serde_json::from_str(
            r#"{"protocol_version":{"major":1,"minor":0},"bundle_format_version":1,"entry_point":"index.md","files":[{"path":"index.md","size":42,"sha256":"not-validated-legacy-metadata"}]}"#,
        )
        .unwrap();

        assert_eq!(manifest.files[0].size, 42);
        let serialized = serde_json::to_string(&manifest).unwrap();
        assert!(!serialized.contains("sha256"));
    }

    #[test]
    fn inbox_response_json_is_stable() {
        let response = InboxManifestResponse {
            protocol_version: version(),
            state: InboxManifestState::NotModified {
                revision: InboxRevision::new(7),
                etag: EntityTag::new("rev-7").unwrap(),
            },
        };

        assert_eq!(
            serde_json::to_string(&response).unwrap(),
            r#"{"protocol_version":{"major":1,"minor":0},"state":{"kind":"not_modified","revision":7,"etag":"rev-7"}}"#
        );
    }

    #[test]
    fn authorization_start_keeps_secret_out_of_public_request() {
        let start = AuthorizationStart {
            protocol_version: version(),
            request: AuthorizationRequest {
                protocol_version: version(),
                request_id: request_id("auth-1"),
                kind: AuthorizationKind::Reader,
                credential_name: None,
                approval_url: "https://reader.example/a/auth-1".into(),
                created_at: Timestamp::new(100),
                expires_at: Timestamp::new(160),
            },
            polling_secret: PollingSecret::new("secret-123").unwrap(),
        };
        let public_json = serde_json::to_string(&start.request).unwrap();

        assert!(!public_json.contains("secret-123"));
        assert_eq!(
            serde_json::to_string(&start).unwrap(),
            r#"{"protocol_version":{"major":1,"minor":0},"request":{"protocol_version":{"major":1,"minor":0},"request_id":"auth-1","kind":"reader","approval_url":"https://reader.example/a/auth-1","created_at":100,"expires_at":160},"polling_secret":"secret-123"}"#
        );
    }

    #[test]
    fn sender_authorization_request_carries_name_but_not_polling_secret() {
        let request = AuthorizationRequest {
            protocol_version: version(),
            request_id: request_id("auth-sender"),
            kind: AuthorizationKind::Sender,
            credential_name: Some(sender_name("laptop")),
            approval_url: "https://reader.example/a/auth-sender".into(),
            created_at: Timestamp::new(100),
            expires_at: Timestamp::new(160),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("laptop"));
        assert!(!json.contains("polling_secret"));
        assert!(!json.contains("secret-123"));
    }

    #[test]
    fn legacy_reader_authorization_request_defaults_missing_credential_name() {
        let request: AuthorizationRequest = serde_json::from_str(
            r#"{
                "protocol_version": {"major": 1, "minor": 0},
                "request_id": "auth-legacy-reader",
                "kind": "reader",
                "approval_url": "https://reader.example/a/auth-legacy-reader",
                "created_at": 100,
                "expires_at": 160
            }"#,
        )
        .unwrap();

        assert_eq!(request.kind, AuthorizationKind::Reader);
        assert_eq!(request.credential_name, None);
    }

    #[test]
    fn claim_result_json_separates_sender_and_reader_scopes() {
        let sender = PollingSecretClaimResult {
            protocol_version: version(),
            outcome: PollingSecretClaimOutcome::Sender {
                credential: SenderCredentialGrant {
                    bearer_token: BearerToken::new("sender-token").unwrap(),
                    metadata: SenderCredentialMetadata {
                        credential_id: credential_id("cred-1"),
                        name: sender_name("laptop"),
                        created_at: Timestamp::new(100),
                        last_used_at: None,
                        revoked_at: None,
                        scope: SenderScope {
                            capabilities: vec![SenderCapability::UploadBundle],
                        },
                    },
                },
            },
        };
        let reader = PollingSecretClaimResult {
            protocol_version: version(),
            outcome: PollingSecretClaimOutcome::Reader {
                session: ReaderSession {
                    session_id: session_id("session-1"),
                    bearer_token: BearerToken::new("reader-token").unwrap(),
                    issued_at: Timestamp::new(100),
                    expires_at: Some(Timestamp::new(200)),
                    scope: ReaderSessionScope {
                        capabilities: vec![ReaderCapability::ReadManifest],
                    },
                },
            },
        };

        assert_eq!(
            serde_json::to_string(&sender).unwrap(),
            r#"{"protocol_version":{"major":1,"minor":0},"outcome":{"kind":"sender","credential":{"bearer_token":"sender-token","metadata":{"credential_id":"cred-1","name":"laptop","created_at":100,"scope":{"capabilities":["upload_bundle"]}}}}}"#
        );
        assert_eq!(
            serde_json::to_string(&reader).unwrap(),
            r#"{"protocol_version":{"major":1,"minor":0},"outcome":{"kind":"reader","session":{"session_id":"session-1","bearer_token":"reader-token","issued_at":100,"expires_at":200,"scope":{"capabilities":["read_manifest"]}}}}"#
        );
    }

    #[test]
    fn reader_session_expiry_is_exclusive_and_missing_metadata_fails_closed() {
        let session = ReaderSession {
            session_id: session_id("session-1"),
            bearer_token: BearerToken::new("reader-token").unwrap(),
            issued_at: Timestamp::new(100),
            expires_at: Some(Timestamp::new(200)),
            scope: ReaderSessionScope {
                capabilities: vec![ReaderCapability::ReadManifest],
            },
        };
        assert!(session.is_valid_at(Timestamp::new(199)));
        assert!(!session.is_valid_at(Timestamp::new(200)));

        let legacy: ReaderSession = serde_json::from_str(
            r#"{"session_id":"session-1","bearer_token":"reader-token","issued_at":100,"scope":{"capabilities":["read_manifest"]}}"#,
        )
        .unwrap();
        assert!(!legacy.is_valid_at(Timestamp::new(100)));
    }

    #[test]
    fn api_error_json_has_a_stable_machine_shape() {
        let error = ApiError {
            protocol_version: version(),
            error: ApiErrorBody {
                code: ApiErrorCode::UnsupportedVersion,
                message: "upgrade the client".into(),
                request_id: Some(RequestId::new("req-1").unwrap()),
            },
        };

        assert_eq!(
            serde_json::to_string(&error).unwrap(),
            r#"{"protocol_version":{"major":1,"minor":0},"error":{"code":"unsupported_version","message":"upgrade the client","request_id":"req-1"}}"#
        );
    }

    #[test]
    fn compatible_minor_versions_accept_unknown_additive_fields() {
        let json = r#"{
            "protocol_version": {"major": 1, "minor": 0},
            "bundle_format_version": 1,
            "entry_point": "index.md",
            "files": [],
            "future_field": "ignored by an older compatible reader"
        }"#;
        let manifest: Manifest = serde_json::from_str(json).unwrap();

        assert_eq!(manifest.protocol_version, version());
        assert!(manifest.files.is_empty());
        assert!(ProtocolVersion::new(1, 1).is_compatible_with(ProtocolVersion::new(1, 2)));
        assert!(!ProtocolVersion::new(2, 0).is_compatible_with(version()));
    }

    #[test]
    fn missing_protocol_version_is_rejected() {
        let error = serde_json::from_str::<Manifest>(
            r#"{"bundle_format_version":1,"entry_point":"index.md","files":[]}"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("protocol_version"));
    }

    #[test]
    fn constrained_strings_reject_unsafe_values() {
        assert!(BundlePath::new("../index.md").is_err());
        assert!(BundlePath::new("/index.md").is_err());
        assert!(BundlePath::new("chapter\\index.md").is_err());
        assert!(AuthorizationRequestId::new("auth/id").is_err());
        assert!(EntityTag::new("etag value").is_err());
        assert!(PollingSecret::new("secret with spaces").is_err());
        assert!(SenderCredentialName::new("\u{7f}").is_err());
    }

    #[test]
    fn secret_debug_output_is_redacted() {
        let polling = PollingSecret::new("do-not-print").unwrap();
        let bearer = BearerToken::new("do-not-print").unwrap();

        assert_eq!(format!("{polling:?}"), "PollingSecret(REDACTED)");
        assert_eq!(format!("{bearer:?}"), "BearerToken(REDACTED)");
    }
}
