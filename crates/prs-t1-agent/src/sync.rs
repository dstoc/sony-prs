//! PRSync reader authorization and bundle delivery.
//!
//! This module is deliberately independent of the Markdown reader. It owns
//! the short-lived network session, the public QR view, and the filesystem
//! handoff that makes one validated bundle visible to the reader.

use crate::framebuffer::NativeDisplay;
use crate::{display, network};
use prs_sync_bundle::{extract_with_size_limit, BundleError, MAX_BUNDLE_PATH_BYTES};
use prs_sync_protocol::{
    ApiError, ApiErrorCode, AuthorizationKind, AuthorizationStart, InboxManifestState,
    InboxRevision, Manifest, PollingSecretClaimOutcome, PollingSecretClaimRequest,
    PollingSecretClaimResult, ProtocolVersion, ReaderSession, Timestamp,
    CURRENT_BUNDLE_FORMAT_VERSION, CURRENT_PROTOCOL_VERSION, MAX_BUNDLE_SIZE,
};
use reqwest::{Method, Response, Url};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::os::unix::fs::symlink;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::runtime::Builder as RuntimeBuilder;

pub const DEFAULT_ENDPOINT: &str = "https://prs-reader.dstoc.workers.dev";
pub const DEFAULT_LIBRARY_ROOT: &str = "/mnt/prs-reader";
pub const DEFAULT_TMPFS_LIMIT_BYTES: u64 = 48 * 1024 * 1024;
pub const MAX_JSON_RESPONSE_BYTES: u64 = 64 * 1024;
const MAX_POLL_DELAY_SECONDS: u32 = 60;
const TEMP_NAME_LIMIT: usize = 32;
static TEMP_NAME_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncConfig {
    endpoint: Url,
    framebuffer: PathBuf,
    library_root: PathBuf,
    tmpfs_limit_bytes: u64,
}

impl SyncConfig {
    fn parse(args: &[String]) -> Result<Self, SyncError> {
        if args.len() > 2 {
            return Err(SyncError::configuration(
                "sync accepts an optional HTTPS endpoint and framebuffer path",
            ));
        }
        let endpoint_value = args
            .first()
            .cloned()
            .or_else(|| std::env::var("PRS_T1_SYNC_URL").ok())
            .unwrap_or_else(|| DEFAULT_ENDPOINT.to_owned());
        let endpoint = parse_endpoint(&endpoint_value)?;
        let framebuffer = args
            .get(1)
            .cloned()
            .or_else(|| std::env::var("PRS_T1_FRAMEBUFFER").ok())
            .unwrap_or_else(|| "/dev/graphics/fb0".to_owned());
        let library_root = std::env::var("PRS_T1_LIBRARY_ROOT")
            .unwrap_or_else(|_| DEFAULT_LIBRARY_ROOT.to_owned());
        let tmpfs_limit_bytes = std::env::var("PRS_T1_TMPFS_LIMIT_BYTES")
            .ok()
            .map(|value| {
                value.parse::<u64>().map_err(|_| {
                    SyncError::configuration("PRS_T1_TMPFS_LIMIT_BYTES must be an unsigned integer")
                })
            })
            .transpose()?
            .unwrap_or(DEFAULT_TMPFS_LIMIT_BYTES);
        if tmpfs_limit_bytes == 0 {
            return Err(SyncError::configuration(
                "PRS_T1_TMPFS_LIMIT_BYTES must be greater than zero",
            ));
        }
        let library_root = PathBuf::from(library_root);
        if !library_root.is_absolute() {
            return Err(SyncError::configuration(
                "PRS_T1_LIBRARY_ROOT must be an absolute tmpfs path",
            ));
        }
        Ok(Self {
            endpoint,
            framebuffer: PathBuf::from(framebuffer),
            library_root,
            tmpfs_limit_bytes,
        })
    }
}

fn parse_endpoint(value: &str) -> Result<Url, SyncError> {
    let endpoint = if value.contains("://") {
        value.to_owned()
    } else {
        format!("https://{value}")
    };
    let endpoint = Url::parse(&endpoint)
        .map_err(|error| SyncError::configuration(format!("invalid sync endpoint: {error}")))?;
    if endpoint.scheme() != "https" {
        return Err(SyncError::configuration("sync endpoint must use HTTPS"));
    }
    if endpoint.host_str().is_none() {
        return Err(SyncError::configuration(
            "sync endpoint must contain a hostname",
        ));
    }
    if !endpoint.username().is_empty() || endpoint.password().is_some() {
        return Err(SyncError::configuration(
            "sync endpoint must not contain credentials",
        ));
    }
    if endpoint.query().is_some() || endpoint.fragment().is_some() {
        return Err(SyncError::configuration(
            "sync endpoint must not contain a query or fragment",
        ));
    }
    Ok(endpoint)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncState {
    Configuration,
    NetworkLoss,
    AuthorizationFailure,
    AuthorizationExpired,
    SessionRejected,
    StaleObject,
    InvalidBundle,
    TmpfsInsufficient,
    ServerError,
    LocalStorage,
}

impl SyncState {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::NetworkLoss => "network_loss",
            Self::AuthorizationFailure => "authorization_failure",
            Self::AuthorizationExpired => "authorization_expired",
            Self::SessionRejected => "session_rejected",
            Self::StaleObject => "stale_object",
            Self::InvalidBundle => "invalid_bundle",
            Self::TmpfsInsufficient => "tmpfs_insufficient",
            Self::ServerError => "server_error",
            Self::LocalStorage => "local_storage",
        }
    }
}

#[derive(Debug)]
pub(crate) struct SyncError {
    stage: &'static str,
    state: SyncState,
    detail: String,
}

impl SyncError {
    fn new(stage: &'static str, state: SyncState, detail: impl Into<String>) -> Self {
        Self {
            stage,
            state,
            detail: detail.into(),
        }
    }

    fn configuration(detail: impl Into<String>) -> Self {
        Self::new("configuration", SyncState::Configuration, detail)
    }

    fn network(detail: impl Into<String>) -> Self {
        Self::new("https", SyncState::NetworkLoss, detail)
    }

    fn authorization(detail: impl Into<String>) -> Self {
        Self::new("authorization", SyncState::AuthorizationFailure, detail)
    }

    fn expired(detail: impl Into<String>) -> Self {
        Self::new("authorization", SyncState::AuthorizationExpired, detail)
    }

    fn session_rejected(detail: impl Into<String>) -> Self {
        Self::new("session", SyncState::SessionRejected, detail)
    }

    fn server(detail: impl Into<String>) -> Self {
        Self::new("server", SyncState::ServerError, detail)
    }

    fn local(detail: impl Into<String>) -> Self {
        Self::new("storage", SyncState::LocalStorage, detail)
    }

    fn state(&self) -> SyncState {
        self.state
    }
}

impl fmt::Display for SyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "stage={} state={} detail={}",
            self.stage,
            self.state.label(),
            self.detail
        )
    }
}

impl std::error::Error for SyncError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyncOutcome {
    Updated {
        revision: InboxRevision,
        entry_point: String,
    },
    Cleared {
        revision: InboxRevision,
    },
    Unchanged {
        revision: InboxRevision,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ManifestSnapshot {
    Empty {
        revision: InboxRevision,
    },
    Current {
        revision: InboxRevision,
        etag: String,
    },
}

#[derive(Debug)]
pub(crate) struct SyncClient {
    config: SyncConfig,
    session: Option<ReaderSession>,
    snapshot: Option<ManifestSnapshot>,
}

impl SyncClient {
    pub(crate) fn new(config: SyncConfig) -> Self {
        Self {
            config,
            session: None,
            snapshot: None,
        }
    }

    async fn synchronize(
        &mut self,
        transport: &Transport,
        display: &mut NativeDisplay,
    ) -> Result<SyncOutcome, SyncError> {
        fs::create_dir_all(&self.config.library_root)
            .map_err(|error| map_storage_error("could not create library root", error))?;

        let mut reauthorized = false;
        loop {
            let session = match self.session.as_ref() {
                Some(session) if session.is_valid_at(now()?) => session.clone(),
                _ => self.authorize(transport, display).await?,
            };
            self.session = Some(session.clone());
            match self.synchronize_with_session(transport, &session).await {
                Ok(outcome) => return Ok(outcome),
                Err(error) if error.state() == SyncState::SessionRejected && !reauthorized => {
                    self.session = None;
                    reauthorized = true;
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn authorize(
        &mut self,
        transport: &Transport,
        display: &mut NativeDisplay,
    ) -> Result<ReaderSession, SyncError> {
        let body = ReaderAuthorizationBody {
            protocol_version: CURRENT_PROTOCOL_VERSION,
        };
        let response = transport
            .json_request(Method::POST, "/api/v1/authorization/reader", &body)?
            .send()
            .await
            .map_err(|error| {
                SyncError::network(format!("could not create reader authorization: {error}"))
            })?;
        let start: AuthorizationStart =
            decode_json(response, "create reader authorization").await?;
        require_protocol(start.protocol_version, "authorization start")?;
        require_protocol(start.request.protocol_version, "authorization request")?;
        if start.request.kind != AuthorizationKind::Reader {
            return Err(SyncError::authorization(
                "server returned a non-reader authorization request",
            ));
        }
        validate_approval_url(&start.request.approval_url, &start.polling_secret)?;
        display::draw_authorization_qr(display, &start.request.approval_url).map_err(|error| {
            SyncError::local(format!("could not display authorization QR: {error}"))
        })?;
        println!("sync.approval_url={}", start.request.approval_url);
        println!("sync.authorization=requested");

        let expires_at = start.request.expires_at;
        loop {
            if now()?.value() >= expires_at.value() {
                return Err(SyncError::expired("reader authorization request expired"));
            }
            let body = PollingSecretClaimRequest {
                protocol_version: CURRENT_PROTOCOL_VERSION,
                request_id: start.request.request_id.clone(),
                polling_secret: start.polling_secret.clone(),
            };
            let response = transport
                .json_request(Method::POST, "/api/v1/authorization/poll", &body)?
                .send()
                .await
                .map_err(|error| {
                    SyncError::network(format!("authorization poll failed: {error}"))
                })?;
            let result: PollingSecretClaimResult =
                decode_json(response, "authorization poll").await?;
            require_protocol(result.protocol_version, "authorization poll")?;
            match result.outcome {
                PollingSecretClaimOutcome::Pending {
                    retry_after_seconds,
                } => {
                    let retry_after_seconds = retry_after_seconds.min(MAX_POLL_DELAY_SECONDS);
                    tokio::time::sleep(Duration::from_secs(u64::from(retry_after_seconds))).await;
                }
                PollingSecretClaimOutcome::Reader { session } => return require_session(session),
                PollingSecretClaimOutcome::Denied => {
                    return Err(SyncError::authorization("reader authorization was denied"));
                }
                PollingSecretClaimOutcome::Expired => {
                    return Err(SyncError::expired("reader authorization request expired"));
                }
                PollingSecretClaimOutcome::AlreadyClaimed => {
                    return Err(SyncError::authorization(
                        "reader authorization was already claimed",
                    ));
                }
                PollingSecretClaimOutcome::Sender { .. } => {
                    return Err(SyncError::authorization(
                        "server returned a sender credential for a reader request",
                    ));
                }
            }
        }
    }

    async fn synchronize_with_session(
        &mut self,
        transport: &Transport,
        session: &ReaderSession,
    ) -> Result<SyncOutcome, SyncError> {
        let mut request = transport.request(Method::GET, "/api/v1/reader/manifest")?;
        request = request.bearer_auth(session.bearer_token.as_str());
        if let Some(snapshot) = &self.snapshot {
            match snapshot {
                ManifestSnapshot::Empty { revision }
                | ManifestSnapshot::Current { revision, .. } => {
                    request = request.header("If-Revision", revision.value().to_string());
                }
            }
            if let ManifestSnapshot::Current { etag, .. } = snapshot {
                request = request.header("If-None-Match", etag.as_str());
            }
        }
        let response = request
            .send()
            .await
            .map_err(|error| SyncError::network(format!("manifest request failed: {error}")))?;
        let response: prs_sync_protocol::InboxManifestResponse =
            decode_json_with_session(response, "fetch reader manifest").await?;
        require_protocol(response.protocol_version, "manifest response")?;
        match response.state {
            InboxManifestState::Empty { revision } => {
                if self.snapshot == Some(ManifestSnapshot::Empty { revision }) {
                    return Ok(SyncOutcome::Unchanged { revision });
                }
                clear_library(&self.config.library_root).map_err(|error| {
                    map_storage_error("could not clear the current library", error)
                })?;
                self.snapshot = Some(ManifestSnapshot::Empty { revision });
                Ok(SyncOutcome::Cleared { revision })
            }
            InboxManifestState::NotModified { revision, etag } => {
                let Some(ManifestSnapshot::Current {
                    revision: snapshot_revision,
                    etag: snapshot_etag,
                }) = self.snapshot.as_ref()
                else {
                    return Err(SyncError::new(
                        "manifest",
                        SyncState::StaleObject,
                        "server returned not-modified without a local manifest snapshot",
                    ));
                };
                if *snapshot_revision != revision || snapshot_etag != etag.as_str() {
                    return Err(SyncError::new(
                        "manifest",
                        SyncState::StaleObject,
                        "server returned a mismatched conditional manifest",
                    ));
                }
                Ok(SyncOutcome::Unchanged { revision })
            }
            InboxManifestState::Current {
                revision,
                etag,
                manifest,
            } => {
                validate_manifest(&manifest)?;
                if self.snapshot
                    == Some(ManifestSnapshot::Current {
                        revision,
                        etag: etag.as_str().to_owned(),
                    })
                {
                    return Ok(SyncOutcome::Unchanged { revision });
                }
                self.download_and_publish(transport, session, revision, etag.as_str(), manifest)
                    .await
            }
        }
    }

    async fn download_and_publish(
        &mut self,
        transport: &Transport,
        session: &ReaderSession,
        revision: InboxRevision,
        etag: &str,
        manifest: Manifest,
    ) -> Result<SyncOutcome, SyncError> {
        let current = self.config.library_root.join("current");
        let current_size = existing_library_size(&current)
            .map_err(|error| map_storage_error("could not inspect current library", error))?;
        let response = transport
            .request(Method::GET, "/api/v1/reader/bundle")?
            .bearer_auth(session.bearer_token.as_str())
            .send()
            .await
            .map_err(|error| SyncError::network(format!("bundle request failed: {error}")))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let body = read_bounded_body(response, "download reader bundle").await?;
            return Err(map_http_error(
                status,
                &body,
                "download reader bundle",
                true,
            ));
        }
        let content_length = response.content_length();
        if content_length.is_some_and(|length| length > MAX_BUNDLE_SIZE) {
            return Err(SyncError::new(
                "download",
                SyncState::InvalidBundle,
                format!("bundle response exceeds {MAX_BUNDLE_SIZE} bytes"),
            ));
        }
        let archive_bound = content_length.unwrap_or(MAX_BUNDLE_SIZE);
        let required = required_storage_bytes(current_size, archive_bound, 0)?;
        if required > self.config.tmpfs_limit_bytes {
            return Err(SyncError::new(
                "download",
                SyncState::TmpfsInsufficient,
                format!(
                    "download requires {required} bytes but configured tmpfs limit is {}",
                    self.config.tmpfs_limit_bytes
                ),
            ));
        }

        let archive = TempArchive::create(&self.config.library_root)
            .map_err(|error| map_storage_error("could not create bundle staging file", error))?;
        let mut archive_file = archive.file.try_clone().map_err(|error| {
            SyncError::local(format!("could not open bundle staging file: {error}"))
        })?;
        stream_bundle(response, &mut archive_file, archive_bound).await?;
        archive_file.flush().map_err(|error| {
            SyncError::local(format!("could not flush bundle staging file: {error}"))
        })?;
        archive_file.seek(SeekFrom::Start(0)).map_err(|error| {
            SyncError::local(format!("could not rewind bundle staging file: {error}"))
        })?;
        let archive_size = archive_file
            .metadata()
            .map_err(|error| SyncError::local(format!("could not inspect staged bundle: {error}")))?
            .len();
        let staging_limit =
            available_storage_bytes(self.config.tmpfs_limit_bytes, current_size, archive_size)?;

        let extracted = unique_path(&self.config.library_root, "prs-sync-library")
            .map_err(|error| map_storage_error("could not choose library staging path", error))?;
        let extracted_manifest =
            extract_with_size_limit(&mut archive_file, &extracted, staging_limit)
                .map_err(map_bundle_error)?;
        if extracted_manifest != manifest {
            let _ = fs::remove_dir_all(&extracted);
            return Err(SyncError::new(
                "download",
                SyncState::StaleObject,
                "downloaded bundle manifest does not match the requested revision",
            ));
        }
        publish_library(&self.config.library_root, &extracted)
            .map_err(|error| map_storage_error("could not publish bundle", error))?;
        self.snapshot = Some(ManifestSnapshot::Current {
            revision,
            etag: etag.to_owned(),
        });
        Ok(SyncOutcome::Updated {
            revision,
            entry_point: manifest.entry_point.as_str().to_owned(),
        })
    }
}

#[derive(Debug, Serialize)]
struct ReaderAuthorizationBody {
    protocol_version: ProtocolVersion,
}

#[derive(Debug)]
struct Transport {
    client: reqwest::Client,
    endpoint: Url,
}

impl Transport {
    async fn connect(endpoint: &Url) -> Result<Self, SyncError> {
        let host = endpoint
            .host_str()
            .ok_or_else(|| SyncError::configuration("sync endpoint has no hostname"))?;
        let port = endpoint.port_or_known_default().ok_or_else(|| {
            SyncError::configuration("sync endpoint must use a known HTTPS port or specify one")
        })?;
        let addresses = network::resolve_host_addresses(host, port)
            .await
            .map_err(|error| SyncError::new("dns", SyncState::NetworkLoss, error))?;
        let client = network::build_https_client(host, &addresses)
            .map_err(|error| SyncError::new("https", SyncState::NetworkLoss, error))?;
        Ok(Self {
            client,
            endpoint: endpoint.clone(),
        })
    }

    fn request(&self, method: Method, path: &str) -> Result<reqwest::RequestBuilder, SyncError> {
        let url = api_url(&self.endpoint, path)?;
        Ok(self.client.request(method, url))
    }

    fn json_request<T: Serialize>(
        &self,
        method: Method,
        path: &str,
        body: &T,
    ) -> Result<reqwest::RequestBuilder, SyncError> {
        let body = serde_json::to_vec(body).map_err(|error| {
            SyncError::configuration(format!("could not encode {path} request: {error}"))
        })?;
        Ok(self
            .request(method, path)?
            .header("Content-Type", "application/json")
            .body(body))
    }
}

fn api_url(endpoint: &Url, path: &str) -> Result<Url, SyncError> {
    let mut url = endpoint.clone();
    let prefix = endpoint.path().trim_end_matches('/');
    let path = path.trim_start_matches('/');
    let joined = if prefix.is_empty() {
        format!("/{path}")
    } else {
        format!("{prefix}/{path}")
    };
    url.set_path(&joined);
    Ok(url)
}

async fn decode_json<T: DeserializeOwned>(
    response: Response,
    operation: &str,
) -> Result<T, SyncError> {
    let status = response.status().as_u16();
    let body = read_bounded_body(response, operation).await?;
    if !(200..300).contains(&status) {
        return Err(map_http_error(status, &body, operation, false));
    }
    serde_json::from_slice(&body).map_err(|error| {
        SyncError::new(
            "response",
            SyncState::ServerError,
            format!("{operation} returned invalid JSON: {error}"),
        )
    })
}

async fn decode_json_with_session<T: DeserializeOwned>(
    response: Response,
    operation: &str,
) -> Result<T, SyncError> {
    let status = response.status().as_u16();
    let body = read_bounded_body(response, operation).await?;
    if !(200..300).contains(&status) {
        return Err(map_http_error(status, &body, operation, true));
    }
    serde_json::from_slice(&body).map_err(|error| {
        SyncError::new(
            "response",
            SyncState::ServerError,
            format!("{operation} returned invalid JSON: {error}"),
        )
    })
}

async fn read_bounded_body(mut response: Response, operation: &str) -> Result<Vec<u8>, SyncError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_JSON_RESPONSE_BYTES)
    {
        return Err(SyncError::new(
            "response",
            SyncState::ServerError,
            format!("{operation} response exceeds {MAX_JSON_RESPONSE_BYTES} bytes"),
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        SyncError::network(format!("could not read {operation} response: {error}"))
    })? {
        let new_length = body
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| SyncError::server(format!("{operation} response length overflowed")))?;
        if new_length as u64 > MAX_JSON_RESPONSE_BYTES {
            return Err(SyncError::new(
                "response",
                SyncState::ServerError,
                format!("{operation} response exceeds {MAX_JSON_RESPONSE_BYTES} bytes"),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn stream_bundle(
    mut response: Response,
    file: &mut File,
    limit: u64,
) -> Result<(), SyncError> {
    let mut received = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| SyncError::network(format!("could not read bundle response: {error}")))?
    {
        received = received.checked_add(chunk.len() as u64).ok_or_else(|| {
            SyncError::new(
                "download",
                SyncState::InvalidBundle,
                "bundle size overflowed",
            )
        })?;
        if received > limit {
            return Err(SyncError::new(
                "download",
                SyncState::InvalidBundle,
                format!("bundle exceeds {limit} bytes"),
            ));
        }
        file.write_all(&chunk)
            .map_err(|error| map_storage_error("could not stage bundle bytes", error))?;
    }
    Ok(())
}

fn map_http_error(status: u16, body: &[u8], operation: &str, session_request: bool) -> SyncError {
    let code = serde_json::from_slice::<ApiError>(body)
        .ok()
        .map(|error| error.error.code);
    match (session_request, code, status) {
        (true, Some(ApiErrorCode::Unauthorized | ApiErrorCode::Forbidden), _)
        | (true, _, 401 | 403) => SyncError::session_rejected(format!(
            "{operation} rejected the reader session with HTTP {status}"
        )),
        (_, Some(ApiErrorCode::Gone), _) | (_, _, 410) => {
            SyncError::expired(format!("{operation} authorization has expired"))
        }
        (_, Some(ApiErrorCode::Conflict), _) => {
            SyncError::authorization(format!("{operation} authorization was rejected"))
        }
        (_, Some(ApiErrorCode::Unauthorized | ApiErrorCode::Forbidden), _) => {
            SyncError::authorization(format!("{operation} was not authorized"))
        }
        (_, Some(ApiErrorCode::NotFound), _) | (_, _, 404) if operation.contains("bundle") => {
            SyncError::new(
                "download",
                SyncState::StaleObject,
                "the current bundle object is no longer available",
            )
        }
        (_, Some(ApiErrorCode::RateLimited), 429) => {
            SyncError::server(format!("{operation} was rate limited"))
        }
        (_, _, 500..=599) => SyncError::server(format!("{operation} returned HTTP {status}")),
        _ => SyncError::server(format!("{operation} returned HTTP {status}")),
    }
}

fn require_protocol(version: ProtocolVersion, operation: &str) -> Result<(), SyncError> {
    version
        .require_compatible(CURRENT_PROTOCOL_VERSION)
        .map_err(|_| {
            SyncError::new(
                "protocol",
                SyncState::ServerError,
                format!("{operation} used unsupported protocol version"),
            )
        })
}

fn validate_manifest(manifest: &Manifest) -> Result<(), SyncError> {
    require_protocol(manifest.protocol_version, "bundle manifest")?;
    if manifest.bundle_format_version != CURRENT_BUNDLE_FORMAT_VERSION {
        return Err(SyncError::new(
            "manifest",
            SyncState::InvalidBundle,
            format!(
                "bundle format version {} is unsupported",
                manifest.bundle_format_version
            ),
        ));
    }
    if Path::new(manifest.entry_point.as_str())
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("md"))
    {
        return Err(SyncError::new(
            "manifest",
            SyncState::InvalidBundle,
            "bundle entry point is not Markdown",
        ));
    }
    let mut paths = HashSet::with_capacity(manifest.files.len());
    if manifest.files.iter().any(|file| {
        file.path.as_str().len() > MAX_BUNDLE_PATH_BYTES
            || file.path.as_str() == "manifest.json"
            || !paths.insert(file.path.as_str())
    }) {
        return Err(SyncError::new(
            "manifest",
            SyncState::InvalidBundle,
            "bundle manifest contains an unsafe or duplicate path",
        ));
    }
    if !manifest
        .files
        .iter()
        .any(|file| file.path == manifest.entry_point)
    {
        return Err(SyncError::new(
            "manifest",
            SyncState::InvalidBundle,
            "bundle entry point is not listed in the manifest",
        ));
    }
    manifest_files_size(manifest)?;
    Ok(())
}

fn manifest_files_size(manifest: &Manifest) -> Result<u64, SyncError> {
    let size = manifest
        .files
        .iter()
        .try_fold(0u64, |total, file| total.checked_add(file.size))
        .ok_or_else(|| {
            SyncError::new(
                "manifest",
                SyncState::InvalidBundle,
                "manifest file sizes overflowed",
            )
        })?;
    if size > MAX_BUNDLE_SIZE {
        return Err(SyncError::new(
            "manifest",
            SyncState::InvalidBundle,
            format!("manifest file sizes exceed {MAX_BUNDLE_SIZE} bytes"),
        ));
    }
    Ok(size)
}

fn required_storage_bytes(
    current_size: u64,
    archive_size: u64,
    extracted_size: u64,
) -> Result<u64, SyncError> {
    current_size
        .checked_add(archive_size)
        .and_then(|value| value.checked_add(extracted_size))
        .ok_or_else(|| {
            SyncError::new(
                "download",
                SyncState::TmpfsInsufficient,
                "tmpfs requirement overflowed",
            )
        })
}

fn available_storage_bytes(
    limit: u64,
    current_size: u64,
    archive_size: u64,
) -> Result<u64, SyncError> {
    let used = required_storage_bytes(current_size, archive_size, 0)?;
    limit.checked_sub(used).ok_or_else(|| {
        SyncError::new(
            "download",
            SyncState::TmpfsInsufficient,
            format!(
                "download requires at least {used} bytes but configured tmpfs limit is {limit}"
            ),
        )
    })
}

fn validate_approval_url(
    approval_url: &str,
    polling_secret: &prs_sync_protocol::PollingSecret,
) -> Result<(), SyncError> {
    let url = Url::parse(approval_url)
        .map_err(|_| SyncError::authorization("server returned an invalid approval URL"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || approval_url.contains(polling_secret.as_str())
    {
        return Err(SyncError::authorization(
            "approval URL must be public HTTPS data and must not contain the polling secret",
        ));
    }
    Ok(())
}

fn require_session(session: ReaderSession) -> Result<ReaderSession, SyncError> {
    let Some(expires_at) = session.expires_at else {
        return Err(SyncError::authorization(
            "reader session did not include a server expiry",
        ));
    };
    if !session
        .scope
        .capabilities
        .contains(&prs_sync_protocol::ReaderCapability::ReadManifest)
        || !session
            .scope
            .capabilities
            .contains(&prs_sync_protocol::ReaderCapability::DownloadBundle)
    {
        return Err(SyncError::authorization(
            "reader session lacks the required read-only capabilities",
        ));
    }
    if expires_at.value() <= now()?.value() {
        return Err(SyncError::expired("reader session is already expired"));
    }
    Ok(session)
}

fn now() -> Result<Timestamp, SyncError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            SyncError::configuration(format!("system clock is before Unix epoch: {error}"))
        })?;
    Ok(Timestamp::new(duration.as_secs()))
}

#[derive(Debug)]
struct TempArchive {
    path: PathBuf,
    file: File,
}

impl TempArchive {
    fn create(parent: &Path) -> io::Result<Self> {
        for _ in 0..TEMP_NAME_LIMIT {
            let path = unique_path(parent, "prs-sync-download")?;
            match OpenOptions::new()
                .write(true)
                .read(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => return Ok(Self { path, file }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique bundle staging path",
        ))
    }
}

impl Drop for TempArchive {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn unique_path(parent: &Path, prefix: &str) -> io::Result<PathBuf> {
    let counter = TEMP_NAME_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    Ok(parent.join(format!(".{prefix}-{pid}-{counter}")))
}

fn publish_library(root: &Path, extracted: &Path) -> io::Result<()> {
    let current = root.join("current");
    let old_target = match fs::symlink_metadata(&current) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = fs::read_link(&current)?;
            safe_generation_target(root, &target)
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "library current path exists and is not the PRSync symlink",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let target = extracted.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "library staging path has no name",
        )
    })?;
    let temporary_link = unique_path(root, "prs-sync-current")?;
    symlink(target, &temporary_link)?;
    if let Err(error) = fs::rename(&temporary_link, &current) {
        let _ = fs::remove_file(&temporary_link);
        return Err(error);
    }
    if let Some(old_target) = old_target {
        if old_target != extracted {
            let _ = fs::remove_dir_all(old_target);
        }
    }
    Ok(())
}

fn clear_library(root: &Path) -> io::Result<()> {
    let current = root.join("current");
    let metadata = match fs::symlink_metadata(&current) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "library current path exists and is not the PRSync symlink",
        ));
    }
    let target = safe_generation_target(root, &fs::read_link(&current)?).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "library current symlink points outside the PRSync root",
        )
    })?;
    let tombstone = unique_path(root, "prs-sync-clearing")?;
    fs::rename(&current, &tombstone)?;
    if let Err(error) = fs::remove_dir_all(&target) {
        let _ = fs::rename(&tombstone, &current);
        return Err(error);
    }
    fs::remove_file(tombstone)
}

fn safe_generation_target(root: &Path, target: &Path) -> Option<PathBuf> {
    if target.is_absolute()
        || target
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(root.join(target))
}

fn existing_library_size(current: &Path) -> io::Result<u64> {
    let metadata = match fs::symlink_metadata(current) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "library current path exists and is not the PRSync symlink",
        ));
    }
    let root = current.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "library current path has no parent",
        )
    })?;
    let target = fs::read_link(current)?;
    let target = safe_generation_target(root, &target).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "library current symlink points outside the PRSync root",
        )
    })?;
    directory_size(&target)
}

fn directory_size(path: &Path) -> io::Result<u64> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "library contains an unexpected symlink",
        ));
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    if !metadata.is_dir() {
        return Ok(0);
    }
    let mut total = 0u64;
    for entry in fs::read_dir(path)? {
        total = total
            .checked_add(directory_size(&entry?.path())?)
            .ok_or_else(|| io::Error::other("library size overflowed"))?;
    }
    Ok(total)
}

fn map_bundle_error(error: BundleError) -> SyncError {
    let detail = error.to_string();
    let state = match &error {
        BundleError::Io(error) if is_storage_capacity_error(error) => SyncState::TmpfsInsufficient,
        BundleError::StagingSizeTooLarge { .. } => SyncState::TmpfsInsufficient,
        _ => SyncState::InvalidBundle,
    };
    SyncError::new("bundle", state, detail)
}

fn map_storage_error(operation: &str, error: io::Error) -> SyncError {
    let state = if is_storage_capacity_error(&error) {
        SyncState::TmpfsInsufficient
    } else {
        SyncState::LocalStorage
    };
    SyncError::new("storage", state, format!("{operation}: {error}"))
}

fn is_storage_capacity_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::OutOfMemory | io::ErrorKind::StorageFull | io::ErrorKind::WriteZero
    )
}

pub(crate) fn run(args: Vec<String>) -> io::Result<()> {
    let config = SyncConfig::parse(&args).map_err(io::Error::other)?;
    crate::wifi::run_sync(config)
}

pub(crate) fn run_active(config: SyncConfig) -> io::Result<()> {
    let mut display = NativeDisplay::open(&config.framebuffer)
        .map_err(|error| io::Error::other(format!("could not open sync display: {error}")))?;
    crate::status::ensure_native_ownership()?;
    let runtime = RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| io::Error::other(format!("could not create sync runtime: {error}")))?;
    let result = runtime.block_on(async {
        crate::tls::initialize().map_err(|error| SyncError::network(error.to_string()))?;
        let transport = Transport::connect(&config.endpoint).await?;
        let mut client = SyncClient::new(config);
        client.synchronize(&transport, &mut display).await
    });
    match result {
        Ok(SyncOutcome::Updated {
            revision,
            entry_point,
        }) => {
            println!("sync.result=updated");
            println!("sync.revision={}", revision.value());
            println!("sync.entry_point={entry_point}");
            Ok(())
        }
        Ok(SyncOutcome::Cleared { revision }) => {
            println!("sync.result=cleared");
            println!("sync.revision={}", revision.value());
            Ok(())
        }
        Ok(SyncOutcome::Unchanged { revision }) => {
            println!("sync.result=unchanged");
            println!("sync.revision={}", revision.value());
            Ok(())
        }
        Err(error) => {
            println!("sync.result=failure");
            println!("sync.failure_stage={}", error.stage);
            println!("sync.failure_kind={}", error.state.label());
            Err(io::Error::other(error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn endpoint_validation_keeps_api_base_path() {
        let endpoint = parse_endpoint("https://reader.example.test/sync").unwrap();
        assert_eq!(
            api_url(&endpoint, "/api/v1/reader/manifest")
                .unwrap()
                .as_str(),
            "https://reader.example.test/sync/api/v1/reader/manifest"
        );
    }

    #[test]
    fn endpoint_rejects_credentials_and_non_https() {
        assert!(parse_endpoint("http://reader.example.test").is_err());
        assert!(parse_endpoint("https://user:secret@reader.example.test").is_err());
        assert!(parse_endpoint("https://reader.example.test?secret=1").is_err());
    }

    #[test]
    fn approval_url_does_not_accept_polling_secret() {
        let secret = prs_sync_protocol::PollingSecret::new("polling-secret").unwrap();
        assert!(validate_approval_url("https://reader.example.test/a/one", &secret).is_ok());
        assert!(
            validate_approval_url("https://reader.example.test/a/polling-secret", &secret).is_err()
        );
    }

    #[test]
    fn service_failures_remain_structured_and_secret_free() {
        let session = map_http_error(401, b"", "fetch reader manifest", true);
        assert_eq!(session.state(), SyncState::SessionRejected);
        assert!(!session.to_string().contains("bearer"));

        let stale = map_http_error(404, b"", "download reader bundle", true);
        assert_eq!(stale.state(), SyncState::StaleObject);

        let expired = map_http_error(410, b"", "authorization poll", false);
        assert_eq!(expired.state(), SyncState::AuthorizationExpired);
    }

    #[test]
    fn old_library_symlink_targets_cannot_escape_the_tmpfs_root() {
        let root = Path::new("/mnt/prs-reader");
        assert!(safe_generation_target(root, Path::new(".generation-1")).is_some());
        assert!(safe_generation_target(root, Path::new("../persistent")).is_none());
        assert!(safe_generation_target(root, Path::new("/persistent")).is_none());
    }

    #[test]
    fn current_size_does_not_follow_an_external_symlink() {
        let root = std::env::temp_dir().join(format!(
            "prs-t1-sync-size-test-{}-{}",
            std::process::id(),
            TEMP_NAME_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let outside_name = format!("prs-t1-sync-outside-{}", std::process::id());
        let outside = root.parent().unwrap().join(&outside_name);
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(Path::new("..").join(&outside_name), root.join("current")).unwrap();

        let error = existing_library_size(&root.join("current")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn library_publish_replaces_only_the_current_symlink() {
        let root = std::env::temp_dir().join(format!(
            "prs-t1-sync-test-{}-{}",
            std::process::id(),
            TEMP_NAME_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let first = root.join(".first");
        let second = root.join(".second");
        fs::create_dir(&first).unwrap();
        fs::write(first.join("index.md"), b"first").unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(second.join("index.md"), b"second").unwrap();
        publish_library(&root, &first).unwrap();
        publish_library(&root, &second).unwrap();
        assert_eq!(fs::read(root.join("current/index.md")).unwrap(), b"second");
        assert!(!first.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clearing_the_library_removes_current_and_its_generation() {
        let root = std::env::temp_dir().join(format!(
            "prs-t1-sync-clear-test-{}-{}",
            std::process::id(),
            TEMP_NAME_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let generation = root.join(".generation");
        fs::create_dir(&generation).unwrap();
        fs::write(generation.join("index.md"), b"current").unwrap();
        publish_library(&root, &generation).unwrap();

        clear_library(&root).unwrap();

        assert!(!root.join("current").exists());
        assert!(!generation.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_library_clear_restores_the_current_symlink() {
        let root = std::env::temp_dir().join(format!(
            "prs-t1-sync-clear-rollback-test-{}-{}",
            std::process::id(),
            TEMP_NAME_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        symlink(Path::new(".missing-generation"), root.join("current")).unwrap();

        let error = clear_library(&root).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            fs::read_link(root.join("current")).unwrap(),
            Path::new(".missing-generation")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tmpfs_budget_includes_the_current_archive_and_extracted_library() {
        assert_eq!(required_storage_bytes(10, 20, 30).unwrap(), 60);
        assert_eq!(
            required_storage_bytes(u64::MAX, 1, 1).unwrap_err().state(),
            SyncState::TmpfsInsufficient
        );
    }

    #[test]
    fn staging_capacity_failure_leaves_current_library_unchanged() {
        let root = std::env::temp_dir().join(format!(
            "prs-t1-sync-staging-limit-test-{}-{}",
            std::process::id(),
            TEMP_NAME_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let generation = root.join(".current-generation");
        fs::create_dir(&generation).unwrap();
        fs::write(generation.join("index.md"), b"current").unwrap();
        publish_library(&root, &generation).unwrap();

        let source = root.join("source-index.md");
        fs::write(&source, b"candidate").unwrap();
        let mut archive = Vec::new();
        let manifest = prs_sync_bundle::BundleBuilder::new(&source)
            .write(&mut archive)
            .unwrap();
        let limit = serde_json::to_vec(&manifest).unwrap().len() as u64;
        let staging = root.join(".candidate-generation");

        let error = extract_with_size_limit(Cursor::new(archive), &staging, limit).unwrap_err();

        assert!(matches!(error, BundleError::StagingSizeTooLarge { .. }));
        assert_eq!(fs::read(root.join("current/index.md")).unwrap(), b"current");
        assert!(!staging.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_size_is_checked_before_download() {
        let manifest = Manifest {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            bundle_format_version: CURRENT_BUNDLE_FORMAT_VERSION,
            entry_point: prs_sync_protocol::BundlePath::new("index.md").unwrap(),
            files: vec![prs_sync_protocol::ManifestFile {
                path: prs_sync_protocol::BundlePath::new("index.md").unwrap(),
                size: MAX_BUNDLE_SIZE + 1,
            }],
        };
        let error = validate_manifest(&manifest).unwrap_err();
        assert_eq!(error.state(), SyncState::InvalidBundle);
    }
}
