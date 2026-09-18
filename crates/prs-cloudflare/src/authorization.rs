//! Authorization state and capability handling for the PRSync Worker.
//!
//! This module owns the security-sensitive part of the Worker boundary. It
//! stores only hashes of polling secrets and bearer tokens. The only method
//! that returns a newly-created bearer token is a successful polling claim.
//! Human approval receives an [`OwnerApprovalCapability`], not a client token.

use crate::identity::TrustedPrincipal;
use prs_sync_protocol::{
    AuthorizationKind, AuthorizationRequest, AuthorizationRequestId, AuthorizationStart,
    AuthorizationState, AuthorizationStatus, BearerToken, CredentialId, PollingSecret,
    ReaderCapability, ReaderSession, ReaderSessionScope, SenderCapability, SenderCredentialGrant,
    SenderCredentialMetadata, SenderCredentialName, SenderScope, SessionId, Timestamp,
    CURRENT_PROTOCOL_VERSION,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fmt;
use worker::js_sys::{self, Function, Uint8Array};
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::{D1Database, D1Result, D1Type};

const SECRET_BYTES: usize = 32;
const DEFAULT_REQUEST_TTL_SECONDS: u64 = 15 * 60;
const DEFAULT_READER_SESSION_TTL_SECONDS: u64 = 30 * 24 * 60 * 60;
const DEFAULT_RETRY_AFTER_SECONDS: u32 = 2;

/// Worker configuration for authorization requests and boot-scoped sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationConfig {
    /// Base URL for the human-facing approval application.
    pub approval_base_url: String,
    /// Lifetime of a pending request.
    pub request_ttl_seconds: u64,
    /// Maximum server-side lifetime of a reader session. The reader still
    /// discards its session when it reboots.
    pub reader_session_ttl_seconds: u64,
    /// Suggested delay for a pending poll.
    pub retry_after_seconds: u32,
}

impl Default for AuthorizationConfig {
    fn default() -> Self {
        Self {
            approval_base_url: "https://reader.example.com".to_owned(),
            request_ttl_seconds: DEFAULT_REQUEST_TTL_SECONDS,
            reader_session_ttl_seconds: DEFAULT_READER_SESSION_TTL_SECONDS,
            retry_after_seconds: DEFAULT_RETRY_AFTER_SECONDS,
        }
    }
}

impl AuthorizationConfig {
    fn approval_url(&self, request_id: &AuthorizationRequestId) -> String {
        format!(
            "{}/a/{}",
            self.approval_base_url.trim_end_matches('/'),
            request_id.as_str()
        )
    }
}

/// A capability produced only after the trusted owner-identity boundary has
/// authenticated the configured owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerApprovalCapability {
    _private: (),
}

impl OwnerApprovalCapability {
    /// This constructor is crate-visible so the Access/test identity boundary
    /// can mint the capability after it has performed its own validation.
    #[allow(dead_code)]
    pub(crate) const fn new() -> Self {
        Self { _private: () }
    }
}

/// A pending-poll capability is tied to one public request identifier. The
/// corresponding high-entropy [`PollingSecret`] remains separate and private.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPollingCapability {
    request_id: AuthorizationRequestId,
    polling_secret: PollingSecret,
}

impl PendingPollingCapability {
    pub fn new(request_id: AuthorizationRequestId, polling_secret: PollingSecret) -> Self {
        Self {
            request_id,
            polling_secret,
        }
    }

    pub fn from_start(start: &AuthorizationStart) -> Self {
        Self::new(
            start.request.request_id.clone(),
            start.polling_secret.clone(),
        )
    }

    pub fn request_id(&self) -> &AuthorizationRequestId {
        &self.request_id
    }

    pub fn polling_secret(&self) -> &PollingSecret {
        &self.polling_secret
    }
}

/// A sender bearer token resolves to sender operations only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderAuthorization {
    pub credential_id: CredentialId,
    pub scope: SenderScope,
}

impl SenderAuthorization {
    pub fn allows(&self, capability: SenderCapability) -> bool {
        self.scope.capabilities.contains(&capability)
    }
}

/// A reader bearer token resolves to read-only operations only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderAuthorization {
    pub session_id: SessionId,
    pub scope: ReaderSessionScope,
}

impl ReaderAuthorization {
    pub fn allows(&self, capability: ReaderCapability) -> bool {
        self.scope.capabilities.contains(&capability)
    }
}

/// The capability classes understood by later Worker route layers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthenticatedCapability {
    PendingPolling(PendingPollingCapability),
    Sender(SenderAuthorization),
    OwnerApproval(OwnerApprovalCapability),
    Reader(ReaderAuthorization),
}

/// A stable failure class for authorization operations. HTTP route code can
/// map these failures to protocol API error codes without inspecting messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationFailure {
    NotFound,
    CredentialAlreadyExists,
    WrongAuthorizationKind,
    InvalidPollingSecret,
    Expired,
    Denied,
    AlreadyClaimed,
    InvalidState,
    Unauthorized,
}

impl fmt::Display for AuthorizationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotFound => "authorization request or credential not found",
            Self::CredentialAlreadyExists => "an active sender credential already uses this name",
            Self::WrongAuthorizationKind => "authorization kind does not match the operation",
            Self::InvalidPollingSecret => "invalid polling secret",
            Self::Expired => "authorization request has expired",
            Self::Denied => "authorization request was denied",
            Self::AlreadyClaimed => "authorization request was already claimed",
            Self::InvalidState => "authorization request has an invalid state",
            Self::Unauthorized => "authorization capability is not valid",
        })
    }
}

impl std::error::Error for AuthorizationFailure {}

/// Errors returned by the authorization service. Route code can inspect the
/// typed failure and then convert infrastructure errors to a Worker error.
#[derive(Debug)]
pub enum AuthorizationError {
    Authorization(AuthorizationFailure),
    Worker(worker::Error),
}

pub type AuthorizationResult<T> = std::result::Result<T, AuthorizationError>;
type Result<T> = AuthorizationResult<T>;

impl AuthorizationError {
    pub fn failure(&self) -> Option<AuthorizationFailure> {
        match self {
            Self::Authorization(failure) => Some(*failure),
            Self::Worker(_) => None,
        }
    }

    pub fn into_worker(self) -> worker::Error {
        match self {
            Self::Authorization(failure) => worker::Error::RustError(failure.to_string()),
            Self::Worker(error) => error,
        }
    }
}

impl fmt::Display for AuthorizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Authorization(failure) => failure.fmt(formatter),
            Self::Worker(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for AuthorizationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Authorization(failure) => Some(failure),
            Self::Worker(error) => Some(error),
        }
    }
}

impl From<AuthorizationFailure> for AuthorizationError {
    fn from(failure: AuthorizationFailure) -> Self {
        Self::Authorization(failure)
    }
}

impl From<worker::Error> for AuthorizationError {
    fn from(error: worker::Error) -> Self {
        Self::Worker(error)
    }
}

impl From<JsValue> for AuthorizationError {
    fn from(value: JsValue) -> Self {
        Self::Worker(worker::Error::from(value))
    }
}

/// D1-backed authorization state machine.
#[derive(Debug)]
pub struct AuthorizationService {
    database: D1Database,
    config: AuthorizationConfig,
}

/// Non-secret request data rendered by the human approval application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequestDetails {
    pub request_id: AuthorizationRequestId,
    pub kind: AuthorizationKind,
    pub credential_name: Option<SenderCredentialName>,
    pub approval_url: String,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub state: AuthorizationState,
}

impl AuthorizationService {
    pub fn new(database: D1Database, config: AuthorizationConfig) -> Self {
        Self { database, config }
    }

    /// Create a pending sender authorization request. The sender name is
    /// persisted with the request and is never taken from the approval URL.
    pub async fn create_sender(
        &self,
        credential_name: &SenderCredentialName,
        now: Timestamp,
    ) -> Result<AuthorizationStart> {
        let args = [D1Type::Text(credential_name.as_str())];
        if self
            .database
            .prepare(
                "SELECT credential_id
                 FROM sender_credentials
                 WHERE name = ?1 AND revoked_at IS NULL",
            )
            .bind_refs(args.iter())?
            .first::<CredentialRow>(None)
            .await?
            .is_some()
        {
            return Err(AuthorizationFailure::CredentialAlreadyExists.into());
        }
        self.create(AuthorizationKind::Sender, Some(credential_name), now)
            .await
    }

    /// Create a pending reader authorization request.
    pub async fn create_reader(&self, now: Timestamp) -> Result<AuthorizationStart> {
        self.create(AuthorizationKind::Reader, None, now).await
    }

    /// Return public request status. This method never returns a polling
    /// secret or a bearer token.
    pub async fn status(
        &self,
        request_id: &AuthorizationRequestId,
        now: Timestamp,
    ) -> Result<AuthorizationStatus> {
        self.expire_if_needed(request_id, now).await?;
        let row = self.load_request(request_id).await?;
        row.status()
    }

    /// Return the non-secret context needed by the approval page. This method
    /// never returns the polling-secret hash, polling secret, or bearer token.
    pub async fn approval_details(
        &self,
        request_id: &AuthorizationRequestId,
        now: Timestamp,
    ) -> Result<ApprovalRequestDetails> {
        self.expire_if_needed(request_id, now).await?;
        self.load_request(request_id).await?.approval_details()
    }

    /// Resolve a platform-authenticated principal against the one configured
    /// owner identity. The resulting capability carries no bearer token and
    /// cannot be constructed from a request ID or approval URL.
    pub async fn authenticate_owner(
        &self,
        principal: &TrustedPrincipal,
    ) -> Result<OwnerApprovalCapability> {
        let row = self
            .database
            .prepare(
                "SELECT issuer, subject, email\n                 FROM owner_identity\n                 WHERE singleton = 1",
            )
            .first::<OwnerIdentityRow>(None)
            .await?
            .ok_or(AuthorizationFailure::Unauthorized)?;
        let email_matches = row
            .email
            .as_deref()
            .map(|configured| principal.email() == Some(configured))
            .unwrap_or(true);
        if row.issuer != principal.issuer() || row.subject != principal.subject() || !email_matches
        {
            return Err(AuthorizationFailure::Unauthorized.into());
        }
        Ok(OwnerApprovalCapability::new())
    }

    /// Approve a sender request after the trusted owner boundary succeeds.
    pub async fn approve_sender(
        &self,
        _owner: OwnerApprovalCapability,
        request_id: &AuthorizationRequestId,
        now: Timestamp,
    ) -> Result<()> {
        self.transition(request_id, AuthorizationKind::Sender, true, now)
            .await
    }

    /// Approve a reader request after the trusted owner boundary succeeds.
    pub async fn approve_reader(
        &self,
        _owner: OwnerApprovalCapability,
        request_id: &AuthorizationRequestId,
        now: Timestamp,
    ) -> Result<()> {
        self.transition(request_id, AuthorizationKind::Reader, true, now)
            .await
    }

    pub async fn deny_sender(
        &self,
        _owner: OwnerApprovalCapability,
        request_id: &AuthorizationRequestId,
        now: Timestamp,
    ) -> Result<()> {
        self.transition(request_id, AuthorizationKind::Sender, false, now)
            .await
    }

    pub async fn deny_reader(
        &self,
        _owner: OwnerApprovalCapability,
        request_id: &AuthorizationRequestId,
        now: Timestamp,
    ) -> Result<()> {
        self.transition(request_id, AuthorizationKind::Reader, false, now)
            .await
    }

    /// Expire a pending request. The conditional update makes expiration safe
    /// when it races with approval or denial.
    pub async fn expire(
        &self,
        request_id: &AuthorizationRequestId,
        now: Timestamp,
    ) -> Result<bool> {
        self.expire_if_needed(request_id, now).await
    }

    /// Claim a sender credential with the private polling secret. The D1 batch
    /// inserts the credential and consumes the request as one atomic operation.
    pub async fn claim_sender(
        &self,
        polling: &PendingPollingCapability,
        now: Timestamp,
    ) -> Result<prs_sync_protocol::PollingSecretClaimResult> {
        self.claim(
            polling.request_id(),
            AuthorizationKind::Sender,
            polling.polling_secret(),
            now,
        )
        .await
    }

    /// Claim a read-only reader session with the private polling secret. The
    /// session is server-expiring but is intended to be discarded on reboot
    /// by the reader client.
    pub async fn claim_reader(
        &self,
        polling: &PendingPollingCapability,
        now: Timestamp,
    ) -> Result<prs_sync_protocol::PollingSecretClaimResult> {
        self.claim(
            polling.request_id(),
            AuthorizationKind::Reader,
            polling.polling_secret(),
            now,
        )
        .await
    }

    /// Authenticate a sender token. This query cannot return a reader session.
    pub async fn authenticate_sender(
        &self,
        bearer_token: &BearerToken,
        now: Timestamp,
    ) -> Result<SenderAuthorization> {
        let token_hash = hash_secret(bearer_token.as_str());
        let now = database_timestamp(now)?;
        let args = [D1Type::Blob(&token_hash), D1Type::Integer(now)];
        let result = self
            .database
            .prepare(
                "UPDATE sender_credentials\n                 SET last_used_at = ?2\n                 WHERE bearer_token_hash = ?1 AND revoked_at IS NULL",
            )
            .bind_refs(args.iter())?
            .run()
            .await?;
        if changed_rows(&result)? != 1 {
            return Err(AuthorizationFailure::Unauthorized.into());
        }

        let args = [D1Type::Blob(&token_hash)];
        let row = self
            .database
            .prepare(
                "SELECT credential_id\n                 FROM sender_credentials\n                 WHERE bearer_token_hash = ?1 AND revoked_at IS NULL",
            )
            .bind_refs(args.iter())?
            .first::<CredentialRow>(None)
            .await?
            .ok_or(AuthorizationFailure::Unauthorized)?;
        Ok(SenderAuthorization {
            credential_id: row.credential_id()?,
            scope: sender_scope(),
        })
    }

    /// Authenticate a reader token. This query cannot return sender
    /// credential-management capabilities.
    pub async fn authenticate_reader(
        &self,
        bearer_token: &BearerToken,
        now: Timestamp,
    ) -> Result<ReaderAuthorization> {
        let token_hash = hash_secret(bearer_token.as_str());
        let now = database_timestamp(now)?;
        let args = [D1Type::Blob(&token_hash), D1Type::Integer(now)];
        let row = self
            .database
            .prepare(
                "SELECT session_id\n                 FROM reader_sessions\n                 WHERE bearer_token_hash = ?1\n                   AND revoked_at IS NULL\n                   AND expires_at > ?2",
            )
            .bind_refs(args.iter())?
            .first::<SessionRow>(None)
            .await?
            .ok_or(AuthorizationFailure::Unauthorized)?;
        Ok(ReaderAuthorization {
            session_id: row.session_id()?,
            scope: reader_scope(),
        })
    }

    /// List sender credential metadata without selecting bearer-token hashes.
    /// Revoked credentials remain visible so callers can distinguish a
    /// revoked name from a name that has never existed.
    pub async fn list_sender_credentials(&self) -> Result<Vec<SenderCredentialMetadata>> {
        let rows = self
            .database
            .prepare(
                "SELECT credential_id, name, created_at, last_used_at, revoked_at
                 FROM sender_credentials
                 ORDER BY created_at ASC, credential_id ASC",
            )
            .all()
            .await?
            .results::<CredentialMetadataRow>()?;
        rows.into_iter()
            .map(CredentialMetadataRow::metadata)
            .collect()
    }

    /// Revoke the active credential with the supplied name. The bearer-token
    /// hash is never returned to the route layer or the client.
    pub async fn revoke_sender_credential(
        &self,
        name: &SenderCredentialName,
        now: Timestamp,
    ) -> Result<bool> {
        let now = database_timestamp(now)?;
        let args = [D1Type::Integer(now), D1Type::Text(name.as_str())];
        let result = self
            .database
            .prepare(
                "UPDATE sender_credentials
                 SET revoked_at = ?1
                 WHERE name = ?2 AND revoked_at IS NULL",
            )
            .bind_refs(args.iter())?
            .run()
            .await?;
        changed_rows(&result).map(|changed| changed == 1)
    }

    async fn create(
        &self,
        kind: AuthorizationKind,
        credential_name: Option<&SenderCredentialName>,
        now: Timestamp,
    ) -> Result<AuthorizationStart> {
        if kind == AuthorizationKind::Sender && credential_name.is_none() {
            return Err(AuthorizationFailure::InvalidState.into());
        }
        if kind == AuthorizationKind::Reader && credential_name.is_some() {
            return Err(AuthorizationFailure::InvalidState.into());
        }
        let created_at = database_timestamp(now)?;
        let expires_at = database_timestamp(add_seconds(now, self.config.request_ttl_seconds)?)?;
        let request_id =
            AuthorizationRequestId::new(format!("auth-{}", hex_encode(&random_bytes::<16>()?)))
                .map_err(|error| worker::Error::RustError(error.to_string()))?;
        let secret = PollingSecret::new(hex_encode(&random_bytes::<SECRET_BYTES>()?))
            .map_err(|error| worker::Error::RustError(error.to_string()))?;
        let polling_hash = hash_secret(secret.as_str());
        let kind_text = kind_text(kind);
        let credential_name_text = credential_name.map(SenderCredentialName::as_str);
        let credential_name_value = credential_name_text
            .map(D1Type::Text)
            .unwrap_or(D1Type::Null);
        let approval_url = self.config.approval_url(&request_id);
        let args = [
            D1Type::Text(request_id.as_str()),
            D1Type::Text(kind_text),
            D1Type::Blob(&polling_hash),
            D1Type::Text(&approval_url),
            D1Type::Integer(created_at),
            D1Type::Integer(expires_at),
            credential_name_value,
        ];
        self.database
            .prepare(
                "INSERT INTO authorization_requests\n                 (request_id, kind, polling_secret_hash, approval_url,\n                  state, created_at, expires_at, credential_name)\n                 VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?7)",
            )
            .bind_refs(args.iter())?
            .run()
            .await?;

        Ok(AuthorizationStart {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            request: AuthorizationRequest {
                protocol_version: CURRENT_PROTOCOL_VERSION,
                request_id,
                kind,
                credential_name: credential_name.cloned(),
                approval_url,
                created_at: now,
                expires_at: add_seconds(now, self.config.request_ttl_seconds)?,
            },
            polling_secret: secret,
        })
    }

    async fn transition(
        &self,
        request_id: &AuthorizationRequestId,
        expected_kind: AuthorizationKind,
        approve: bool,
        now: Timestamp,
    ) -> Result<()> {
        let now = database_timestamp(now)?;
        let state = if approve { "approved" } else { "denied" };
        let timestamp_column = if approve { "approved_at" } else { "denied_at" };
        let query = format!(
            "UPDATE authorization_requests\n             SET state = ?1, {timestamp_column} = ?2\n             WHERE request_id = ?3 AND kind = ?4\n               AND state = 'pending' AND expires_at > ?2"
        );
        let kind = kind_text(expected_kind);
        let args = [
            D1Type::Text(state),
            D1Type::Integer(now),
            D1Type::Text(request_id.as_str()),
            D1Type::Text(kind),
        ];
        let result = self
            .database
            .prepare(query)
            .bind_refs(args.iter())?
            .run()
            .await?;
        if changed_rows(&result)? == 1 {
            return Ok(());
        }

        let row = self.load_request(request_id).await?;
        if row.kind()? != expected_kind {
            return Err(AuthorizationFailure::WrongAuthorizationKind.into());
        }
        if row.state()? == RequestState::Pending && row.expires_at <= now {
            self.expire_if_needed(request_id, Timestamp::new(now as u64))
                .await?;
            return Err(AuthorizationFailure::Expired.into());
        }
        Err(match row.state()? {
            RequestState::Denied => AuthorizationFailure::Denied,
            RequestState::Expired => AuthorizationFailure::Expired,
            RequestState::Consumed => AuthorizationFailure::AlreadyClaimed,
            RequestState::Approved => AuthorizationFailure::InvalidState,
            RequestState::Pending => AuthorizationFailure::InvalidState,
        }
        .into())
    }

    async fn claim(
        &self,
        request_id: &AuthorizationRequestId,
        expected_kind: AuthorizationKind,
        polling_secret: &PollingSecret,
        now: Timestamp,
    ) -> Result<prs_sync_protocol::PollingSecretClaimResult> {
        let row = self.load_request(request_id).await?;
        if row.kind()? != expected_kind {
            return Err(AuthorizationFailure::WrongAuthorizationKind.into());
        }
        if !constant_time_eq(
            &row.polling_secret_hash,
            &hash_secret(polling_secret.as_str()),
        ) {
            return Err(AuthorizationFailure::InvalidPollingSecret.into());
        }

        let now_value = now.value();
        match row.state()? {
            RequestState::Pending if now_value >= row.expires_at as u64 => {
                self.expire_if_needed(request_id, now).await?;
                Ok(claim_result(ClaimOutcome::Expired))
            }
            RequestState::Pending => Ok(claim_result(ClaimOutcome::Pending {
                retry_after_seconds: self.config.retry_after_seconds,
            })),
            RequestState::Denied => Ok(claim_result(ClaimOutcome::Denied)),
            RequestState::Expired => Ok(claim_result(ClaimOutcome::Expired)),
            RequestState::Consumed => Ok(claim_result(ClaimOutcome::AlreadyClaimed)),
            RequestState::Approved => {
                if expected_kind == AuthorizationKind::Sender {
                    self.claim_sender_row(row, request_id, now).await
                } else {
                    self.claim_reader_row(row, request_id, now).await
                }
            }
        }
    }

    async fn claim_sender_row(
        &self,
        row: AuthorizationRow,
        request_id: &AuthorizationRequestId,
        now: Timestamp,
    ) -> Result<prs_sync_protocol::PollingSecretClaimResult> {
        let credential_name = row
            .credential_name
            .as_deref()
            .ok_or(AuthorizationFailure::InvalidState)
            .and_then(|name| {
                SenderCredentialName::new(name).map_err(|_| AuthorizationFailure::InvalidState)
            })?;
        let credential_id =
            CredentialId::new(format!("cred-{}", hex_encode(&random_bytes::<16>()?)))
                .map_err(|error| worker::Error::RustError(error.to_string()))?;
        let bearer_token = BearerToken::new(hex_encode(&random_bytes::<SECRET_BYTES>()?))
            .map_err(|error| worker::Error::RustError(error.to_string()))?;
        let token_hash = hash_secret(bearer_token.as_str());
        let now = database_timestamp(now)?;
        let insert_args = [
            D1Type::Text(credential_id.as_str()),
            D1Type::Text(credential_name.as_str()),
            D1Type::Blob(&token_hash),
            D1Type::Integer(now),
            D1Type::Text(request_id.as_str()),
        ];
        let update_args = [
            D1Type::Integer(now),
            D1Type::Text(credential_id.as_str()),
            D1Type::Text(request_id.as_str()),
        ];
        let results = self
            .database
            .batch(vec![
                self.database
                    .prepare(
                        "INSERT INTO sender_credentials\n                         (credential_id, name, bearer_token_hash, created_at)\n                         SELECT ?1, ?2, ?3, ?4\n                         WHERE EXISTS (\n                           SELECT 1 FROM authorization_requests\n                           WHERE request_id = ?5 AND kind = 'sender' AND state = 'approved'\n                         )",
                    )
                    .bind_refs(insert_args.iter())?,
                self.database
                    .prepare(
                        "UPDATE authorization_requests\n                         SET state = 'consumed', consumed_at = ?1, credential_id = ?2\n                         WHERE request_id = ?3 AND kind = 'sender' AND state = 'approved'",
                    )
                    .bind_refs(update_args.iter())?,
            ])
            .await?;
        if changed_rows(&results[1])? != 1 {
            return Ok(claim_result(ClaimOutcome::AlreadyClaimed));
        }
        Ok(claim_result(ClaimOutcome::Sender {
            credential: SenderCredentialGrant {
                bearer_token,
                metadata: SenderCredentialMetadata {
                    credential_id,
                    name: credential_name,
                    created_at: Timestamp::new(now as u64),
                    last_used_at: None,
                    revoked_at: None,
                    scope: sender_scope(),
                },
            },
        }))
    }

    async fn claim_reader_row(
        &self,
        _row: AuthorizationRow,
        request_id: &AuthorizationRequestId,
        now: Timestamp,
    ) -> Result<prs_sync_protocol::PollingSecretClaimResult> {
        let session_id = SessionId::new(format!("session-{}", hex_encode(&random_bytes::<16>()?)))
            .map_err(|error| worker::Error::RustError(error.to_string()))?;
        let bearer_token = BearerToken::new(hex_encode(&random_bytes::<SECRET_BYTES>()?))
            .map_err(|error| worker::Error::RustError(error.to_string()))?;
        let token_hash = hash_secret(bearer_token.as_str());
        let issued_at = database_timestamp(now)?;
        let expires_at =
            database_timestamp(add_seconds(now, self.config.reader_session_ttl_seconds)?)?;
        let insert_args = [
            D1Type::Text(session_id.as_str()),
            D1Type::Blob(&token_hash),
            D1Type::Integer(issued_at),
            D1Type::Integer(expires_at),
            D1Type::Text(request_id.as_str()),
        ];
        let update_args = [
            D1Type::Integer(issued_at),
            D1Type::Text(session_id.as_str()),
            D1Type::Text(request_id.as_str()),
        ];
        let results = self
            .database
            .batch(vec![
                self.database
                    .prepare(
                        "INSERT INTO reader_sessions\n                         (session_id, bearer_token_hash, issued_at, expires_at)\n                         SELECT ?1, ?2, ?3, ?4\n                         WHERE EXISTS (\n                           SELECT 1 FROM authorization_requests\n                           WHERE request_id = ?5 AND kind = 'reader' AND state = 'approved'\n                         )",
                    )
                    .bind_refs(insert_args.iter())?,
                self.database
                    .prepare(
                        "UPDATE authorization_requests\n                         SET state = 'consumed', consumed_at = ?1, session_id = ?2\n                         WHERE request_id = ?3 AND kind = 'reader' AND state = 'approved'",
                    )
                    .bind_refs(update_args.iter())?,
            ])
            .await?;
        if changed_rows(&results[1])? != 1 {
            return Ok(claim_result(ClaimOutcome::AlreadyClaimed));
        }
        Ok(claim_result(ClaimOutcome::Reader {
            session: ReaderSession {
                session_id,
                bearer_token,
                issued_at: Timestamp::new(issued_at as u64),
                scope: reader_scope(),
            },
        }))
    }

    async fn expire_if_needed(
        &self,
        request_id: &AuthorizationRequestId,
        now: Timestamp,
    ) -> Result<bool> {
        let now = database_timestamp(now)?;
        let args = [D1Type::Text(request_id.as_str()), D1Type::Integer(now)];
        let result = self
            .database
            .prepare(
                "UPDATE authorization_requests\n                 SET state = 'expired', expired_at = ?2\n                 WHERE request_id = ?1 AND state = 'pending' AND expires_at <= ?2",
            )
            .bind_refs(args.iter())?
            .run()
            .await?;
        Ok(changed_rows(&result)? == 1)
    }

    async fn load_request(&self, request_id: &AuthorizationRequestId) -> Result<AuthorizationRow> {
        let args = [D1Type::Text(request_id.as_str())];
        self.database
            .prepare(
                "SELECT request_id, kind, credential_name, polling_secret_hash, state,\n                        approval_url, created_at, expires_at\n                 FROM authorization_requests\n                 WHERE request_id = ?1",
            )
            .bind_refs(args.iter())?
            .first(None)
            .await?
            .ok_or_else(|| AuthorizationFailure::NotFound.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestState {
    Pending,
    Approved,
    Denied,
    Expired,
    Consumed,
}

impl RequestState {
    fn from_database(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "denied" => Ok(Self::Denied),
            "expired" => Ok(Self::Expired),
            "consumed" => Ok(Self::Consumed),
            _ => Err(AuthorizationFailure::InvalidState.into()),
        }
    }

    fn protocol(self) -> AuthorizationState {
        match self {
            Self::Pending => AuthorizationState::Pending,
            Self::Approved => AuthorizationState::Approved,
            Self::Denied => AuthorizationState::Denied,
            Self::Expired => AuthorizationState::Expired,
            Self::Consumed => AuthorizationState::Claimed,
        }
    }
}

#[derive(Debug, Deserialize)]
struct AuthorizationRow {
    request_id: String,
    kind: String,
    credential_name: Option<String>,
    polling_secret_hash: Vec<u8>,
    state: String,
    approval_url: String,
    created_at: i32,
    expires_at: i32,
}

impl AuthorizationRow {
    fn id(&self) -> Result<AuthorizationRequestId> {
        Ok(AuthorizationRequestId::new(self.request_id.clone())
            .map_err(|error| worker::Error::RustError(error.to_string()))?)
    }

    fn kind(&self) -> Result<AuthorizationKind> {
        match self.kind.as_str() {
            "sender" => Ok(AuthorizationKind::Sender),
            "reader" => Ok(AuthorizationKind::Reader),
            _ => Err(AuthorizationFailure::InvalidState.into()),
        }
    }

    fn state(&self) -> Result<RequestState> {
        RequestState::from_database(&self.state)
    }

    fn status(&self) -> Result<AuthorizationStatus> {
        Ok(AuthorizationStatus {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            request_id: self.id()?,
            kind: self.kind()?,
            state: self.state()?.protocol(),
            expires_at: Timestamp::new(self.expires_at as u64),
        })
    }

    fn approval_details(&self) -> Result<ApprovalRequestDetails> {
        Ok(ApprovalRequestDetails {
            request_id: self.id()?,
            kind: self.kind()?,
            credential_name: self
                .credential_name
                .as_deref()
                .map(SenderCredentialName::new)
                .transpose()
                .map_err(|error| worker::Error::RustError(error.to_string()))?,
            approval_url: self.approval_url.clone(),
            created_at: Timestamp::new(self.created_at as u64),
            expires_at: Timestamp::new(self.expires_at as u64),
            state: self.state()?.protocol(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct OwnerIdentityRow {
    issuer: String,
    subject: String,
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CredentialRow {
    credential_id: String,
}

impl CredentialRow {
    fn credential_id(&self) -> Result<CredentialId> {
        Ok(CredentialId::new(self.credential_id.clone())
            .map_err(|error| worker::Error::RustError(error.to_string()))?)
    }
}

#[derive(Debug, Deserialize)]
struct CredentialMetadataRow {
    credential_id: String,
    name: String,
    created_at: i32,
    last_used_at: Option<i32>,
    revoked_at: Option<i32>,
}

impl CredentialMetadataRow {
    fn metadata(self) -> Result<SenderCredentialMetadata> {
        Ok(SenderCredentialMetadata {
            credential_id: CredentialId::new(self.credential_id)
                .map_err(|error| worker::Error::RustError(error.to_string()))?,
            name: SenderCredentialName::new(self.name)
                .map_err(|error| worker::Error::RustError(error.to_string()))?,
            created_at: timestamp(self.created_at)?,
            last_used_at: self.last_used_at.map(timestamp).transpose()?,
            revoked_at: self.revoked_at.map(timestamp).transpose()?,
            scope: sender_scope(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct SessionRow {
    session_id: String,
}

impl SessionRow {
    fn session_id(&self) -> Result<SessionId> {
        Ok(SessionId::new(self.session_id.clone())
            .map_err(|error| worker::Error::RustError(error.to_string()))?)
    }
}

#[derive(Debug)]
enum ClaimOutcome {
    Pending { retry_after_seconds: u32 },
    Sender { credential: SenderCredentialGrant },
    Reader { session: ReaderSession },
    Denied,
    Expired,
    AlreadyClaimed,
}

fn claim_result(outcome: ClaimOutcome) -> prs_sync_protocol::PollingSecretClaimResult {
    use prs_sync_protocol::PollingSecretClaimOutcome;

    let outcome = match outcome {
        ClaimOutcome::Pending {
            retry_after_seconds,
        } => PollingSecretClaimOutcome::Pending {
            retry_after_seconds,
        },
        ClaimOutcome::Sender { credential } => PollingSecretClaimOutcome::Sender { credential },
        ClaimOutcome::Reader { session } => PollingSecretClaimOutcome::Reader { session },
        ClaimOutcome::Denied => PollingSecretClaimOutcome::Denied,
        ClaimOutcome::Expired => PollingSecretClaimOutcome::Expired,
        ClaimOutcome::AlreadyClaimed => PollingSecretClaimOutcome::AlreadyClaimed,
    };
    prs_sync_protocol::PollingSecretClaimResult {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        outcome,
    }
}

fn kind_text(kind: AuthorizationKind) -> &'static str {
    match kind {
        AuthorizationKind::Sender => "sender",
        AuthorizationKind::Reader => "reader",
    }
}

fn sender_scope() -> SenderScope {
    SenderScope {
        capabilities: vec![
            SenderCapability::UploadBundle,
            SenderCapability::ClearInbox,
            SenderCapability::ManageCredentials,
        ],
    }
}

fn reader_scope() -> ReaderSessionScope {
    ReaderSessionScope {
        capabilities: vec![
            ReaderCapability::ReadManifest,
            ReaderCapability::DownloadBundle,
        ],
    }
}

fn changed_rows(result: &D1Result) -> Result<usize> {
    Ok(result
        .meta()?
        .and_then(|meta| meta.changes)
        .unwrap_or_default())
}

fn database_timestamp(timestamp: Timestamp) -> Result<i32> {
    Ok(i32::try_from(timestamp.value())
        .map_err(|_| worker::Error::RustError("timestamp does not fit D1 INTEGER".to_owned()))?)
}

fn timestamp(value: i32) -> Result<Timestamp> {
    Ok(Timestamp::new(u64::try_from(value).map_err(|_| {
        worker::Error::RustError("database timestamp is negative".to_owned())
    })?))
}

fn add_seconds(timestamp: Timestamp, seconds: u64) -> Result<Timestamp> {
    Ok(timestamp
        .value()
        .checked_add(seconds)
        .map(Timestamp::new)
        .ok_or_else(|| worker::Error::RustError("authorization timestamp overflow".to_owned()))?)
}

fn hash_secret(secret: &str) -> Vec<u8> {
    Sha256::digest(secret.as_bytes()).to_vec()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

/// Use the Workers runtime's Web Crypto implementation instead of a
/// predictable PRNG or a host-only entropy source.
fn random_bytes<const LENGTH: usize>() -> Result<[u8; LENGTH]> {
    let global = js_sys::global();
    let crypto = js_sys::Reflect::get(&global, &JsValue::from_str("crypto"))?;
    let get_random_values = js_sys::Reflect::get(&crypto, &JsValue::from_str("getRandomValues"))?
        .dyn_into::<Function>()?;
    let values = Uint8Array::new_with_length(LENGTH as u32);
    get_random_values.call1(&crypto, values.as_ref())?;
    let mut bytes = [0u8; LENGTH];
    values.copy_to(&mut bytes);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prs_sync_protocol::{ReaderCapability, SenderCapability};

    fn request_id() -> AuthorizationRequestId {
        AuthorizationRequestId::new("auth-test").unwrap()
    }

    #[test]
    fn approval_url_contains_only_the_public_request_identifier() {
        let config = AuthorizationConfig::default();
        let url = config.approval_url(&request_id());
        assert_eq!(url, "https://reader.example.com/a/auth-test");
        assert!(!url.contains("polling"));
        assert!(!url.contains("secret"));
    }

    #[test]
    fn hashing_is_sha256_and_comparison_is_constant_shape() {
        assert_eq!(
            hex_encode(&hash_secret("polling-secret")),
            "dae5c1e9d7d9dee2ea2a218465d6ad348c0448cc41eb7663f19403b9d3ff626d"
        );
        assert!(constant_time_eq(b"same", b"same"));
        assert!(!constant_time_eq(b"same", b"different"));
    }

    #[test]
    fn capability_scopes_are_disjoint() {
        let sender = SenderAuthorization {
            credential_id: CredentialId::new("credential-1").unwrap(),
            scope: sender_scope(),
        };
        let reader = ReaderAuthorization {
            session_id: SessionId::new("session-1").unwrap(),
            scope: reader_scope(),
        };
        assert!(sender.allows(SenderCapability::UploadBundle));
        assert!(sender.allows(SenderCapability::ManageCredentials));
        assert!(reader.allows(ReaderCapability::ReadManifest));
        assert_eq!(
            reader.scope.capabilities,
            vec![
                ReaderCapability::ReadManifest,
                ReaderCapability::DownloadBundle
            ]
        );
    }

    #[test]
    fn database_states_map_to_protocol_states_without_rewind() {
        assert_eq!(
            RequestState::from_database("pending").unwrap().protocol(),
            AuthorizationState::Pending
        );
        assert_eq!(
            RequestState::from_database("approved").unwrap().protocol(),
            AuthorizationState::Approved
        );
        assert_eq!(
            RequestState::from_database("denied").unwrap().protocol(),
            AuthorizationState::Denied
        );
        assert_eq!(
            RequestState::from_database("expired").unwrap().protocol(),
            AuthorizationState::Expired
        );
        assert_eq!(
            RequestState::from_database("consumed").unwrap().protocol(),
            AuthorizationState::Claimed
        );
        assert!(RequestState::from_database("claimed").is_err());
    }

    #[test]
    fn owner_capability_is_not_a_bearer_token() {
        let capability = AuthenticatedCapability::OwnerApproval(OwnerApprovalCapability::new());
        assert!(matches!(
            capability,
            AuthenticatedCapability::OwnerApproval(_)
        ));
        assert!(!format!("{capability:?}").contains("token"));
    }

    #[test]
    fn pending_polling_capability_redacts_its_secret() {
        let capability = PendingPollingCapability::new(
            request_id(),
            PollingSecret::new("private-polling-secret").unwrap(),
        );
        assert_eq!(capability.request_id().as_str(), "auth-test");
        assert_eq!(
            capability.polling_secret().as_str(),
            "private-polling-secret"
        );
        assert!(!format!("{capability:?}").contains("private-polling-secret"));
    }
}
