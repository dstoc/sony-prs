//! Versioned protocol routes for sender and reader clients.
//!
//! The routes use bearer capabilities issued by the authorization flow. A
//! sender can write the inbox and manage sender credentials. A reader can
//! read the current manifest and bundle only. No protocol route uses the
//! human approval identity boundary.

use crate::authorization::{
    AuthorizationConfig, AuthorizationError, AuthorizationFailure, AuthorizationService,
    PendingPollingCapability, RateLimitDecision,
};
use crate::storage::{BundlePushError, BundleStore, PublishedBundle};
use futures_util::StreamExt;
use prs_sync_protocol::{
    ApiError, ApiErrorBody, ApiErrorCode, AuthorizationKind, AuthorizationRequestId,
    AuthorizationStart, AuthorizationStatus, BearerToken, EntityTag, InboxManifestResponse,
    InboxManifestState, InboxRevision, PollingSecretClaimRequest, PollingSecretClaimResult,
    ProtocolVersion, SenderCredentialMetadata, SenderCredentialName, CURRENT_PROTOCOL_VERSION,
    MAX_BUNDLE_SIZE,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use worker::{Env, Request, Response, ResponseBuilder, Result, RouteContext, Router};

type ApiResult<T> = std::result::Result<T, ApiFailure>;

const MAX_AUTHORIZATION_BODY_SIZE: u64 = 4 * 1024;
const MAX_CREDENTIAL_MANAGEMENT_BODY_SIZE: u64 = 4 * 1024;

#[derive(Debug)]
struct ApiFailure {
    status: u16,
    code: ApiErrorCode,
    message: String,
    retry_after_seconds: Option<u32>,
}

impl ApiFailure {
    fn new(status: u16, code: ApiErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retry_after_seconds: None,
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(400, ApiErrorCode::InvalidRequest, message)
    }

    fn unauthorized() -> Self {
        Self::new(
            401,
            ApiErrorCode::Unauthorized,
            "a valid bearer token is required",
        )
    }

    fn internal() -> Self {
        Self::new(
            500,
            ApiErrorCode::Internal,
            "the Worker could not complete the request",
        )
    }

    fn rate_limited(retry_after_seconds: u32) -> Self {
        Self {
            status: 429,
            code: ApiErrorCode::RateLimited,
            message: "the authorization request rate limit was exceeded".to_owned(),
            retry_after_seconds: Some(retry_after_seconds),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SenderAuthorizationBody {
    protocol_version: ProtocolVersion,
    #[serde(alias = "name")]
    credential_name: SenderCredentialName,
}

#[derive(Debug, Deserialize)]
struct ReaderAuthorizationBody {
    protocol_version: ProtocolVersion,
}

#[derive(Debug, Deserialize)]
struct CredentialRevokeBody {
    protocol_version: ProtocolVersion,
    name: SenderCredentialName,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    protocol_version: ProtocolVersion,
    status: &'static str,
    service: &'static str,
}

#[derive(Debug, Serialize)]
struct CredentialListResponse {
    protocol_version: ProtocolVersion,
    credentials: Vec<SenderCredentialMetadata>,
}

#[derive(Debug, Serialize)]
struct CredentialRevokeResponse {
    protocol_version: ProtocolVersion,
    name: SenderCredentialName,
    revoked: bool,
}

#[derive(Debug, Serialize)]
struct BundleResponse {
    protocol_version: ProtocolVersion,
    revision: InboxRevision,
    etag: EntityTag,
    size_bytes: u64,
}

#[derive(Debug, Serialize)]
struct EmptyInboxResponse {
    protocol_version: ProtocolVersion,
    revision: InboxRevision,
    state: &'static str,
}

#[cfg(feature = "local-test")]
#[derive(Debug, Serialize)]
struct TestMaintenanceResponse {
    authorization: crate::authorization::MaintenanceReport,
    bundles: crate::storage::CleanupReport,
}

/// Adds the public health and versioned protocol routes to a Worker router.
pub fn register(router: Router<'static, ()>) -> Router<'static, ()> {
    let router = router
        .get_async("/", root)
        .get_async("/health", health)
        .post_async("/api/v1/authorization/sender", create_sender)
        .post_async("/api/v1/authorization/reader", create_reader)
        .post_async("/api/v1/authorization/poll", poll_authorization)
        .get_async("/api/v1/authorization/:request_id", authorization_status)
        .put_async("/api/v1/sender/bundle", push_bundle)
        .delete_async("/api/v1/sender/bundle", clear_bundle)
        .get_async("/api/v1/sender/credentials", list_credentials)
        .delete_async("/api/v1/sender/credentials", revoke_credentials)
        .delete_async("/api/v1/sender/credentials/:name", revoke_named_credential)
        .get_async("/api/v1/reader/manifest", reader_manifest)
        .get_async("/api/v1/reader/bundle", reader_bundle);

    #[cfg(feature = "local-test")]
    let router = router.post_async("/__test/maintenance", test_maintenance);

    router
}

async fn root(_request: Request, _context: RouteContext<()>) -> Result<Response> {
    json_response(
        &HealthResponse {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            status: "ok",
            service: "prs-cloudflare",
        },
        200,
    )
}

async fn health(_request: Request, _context: RouteContext<()>) -> Result<Response> {
    root(_request, _context).await
}

#[cfg(feature = "local-test")]
async fn test_maintenance(request: Request, context: RouteContext<()>) -> Result<Response> {
    if !crate::test_support::local_enabled(&context.env) {
        return Response::error("local maintenance route is disabled", 404);
    }
    let current_time = crate::request_timestamp(&request, &context.env);
    let authorization = authorization_service(&context.env)?
        .cleanup(current_time)
        .await
        .map_err(AuthorizationError::into_worker)?;
    let bundles = bundle_store_for_request(&context.env, &request)?
        .cleanup(current_time.value())
        .await?;
    Response::from_json(&TestMaintenanceResponse {
        authorization,
        bundles,
    })
}

async fn create_sender(mut request: Request, context: RouteContext<()>) -> Result<Response> {
    finish(create_sender_inner(&mut request, &context.env).await)
}

async fn create_sender_inner(request: &mut Request, env: &Env) -> ApiResult<AuthorizationStart> {
    let body: SenderAuthorizationBody =
        read_json_body(request, MAX_AUTHORIZATION_BODY_SIZE).await?;
    require_version(body.protocol_version)?;
    let service = authorization_service(env).map_err(internal_error)?;
    enforce_creation_rate_limit(&service, request, env).await?;
    service
        .create_sender(
            &body.credential_name,
            crate::request_timestamp(request, env),
        )
        .await
        .map_err(map_authorization_error)
}

async fn create_reader(mut request: Request, context: RouteContext<()>) -> Result<Response> {
    finish(create_reader_inner(&mut request, &context.env).await)
}

async fn create_reader_inner(request: &mut Request, env: &Env) -> ApiResult<AuthorizationStart> {
    let body: ReaderAuthorizationBody =
        read_json_body(request, MAX_AUTHORIZATION_BODY_SIZE).await?;
    require_version(body.protocol_version)?;
    let service = authorization_service(env).map_err(internal_error)?;
    enforce_creation_rate_limit(&service, request, env).await?;
    service
        .create_reader(crate::request_timestamp(request, env))
        .await
        .map_err(map_authorization_error)
}

async fn poll_authorization(mut request: Request, context: RouteContext<()>) -> Result<Response> {
    finish(poll_authorization_inner(&mut request, &context.env).await)
}

async fn poll_authorization_inner(
    request: &mut Request,
    env: &Env,
) -> ApiResult<PollingSecretClaimResult> {
    let body: PollingSecretClaimRequest =
        read_json_body(request, MAX_AUTHORIZATION_BODY_SIZE).await?;
    require_version(body.protocol_version)?;
    let service = authorization_service(env).map_err(internal_error)?;
    enforce_poll_rate_limit(&service, request, env).await?;
    let current_time = crate::request_timestamp(request, env);
    let status = service
        .status(&body.request_id, current_time)
        .await
        .map_err(map_authorization_error)?;
    let polling = PendingPollingCapability::new(body.request_id, body.polling_secret);
    match status.kind {
        AuthorizationKind::Sender => service
            .claim_sender(&polling, current_time)
            .await
            .map_err(map_authorization_error),
        AuthorizationKind::Reader => service
            .claim_reader(&polling, current_time)
            .await
            .map_err(map_authorization_error),
    }
}

async fn authorization_status(request: Request, context: RouteContext<()>) -> Result<Response> {
    finish(authorization_status_inner(&request, &context.env, context.param("request_id")).await)
}

async fn authorization_status_inner(
    request: &Request,
    env: &Env,
    request_id: Option<&String>,
) -> ApiResult<AuthorizationStatus> {
    let request_id = parse_request_id(request_id)?;
    let service = authorization_service(env).map_err(internal_error)?;
    enforce_poll_rate_limit(&service, request, env).await?;
    let current_time = crate::request_timestamp(request, env);
    service
        .status(&request_id, current_time)
        .await
        .map_err(map_authorization_error)
}

async fn push_bundle(mut request: Request, context: RouteContext<()>) -> Result<Response> {
    finish(push_bundle_inner(&mut request, &context.env).await)
}

async fn push_bundle_inner(request: &mut Request, env: &Env) -> ApiResult<BundleResponse> {
    let sender = authenticate_sender(request, env).await?;
    require_capability(sender, prs_sync_protocol::SenderCapability::UploadBundle)?;
    let bytes = match read_limited_body(request, MAX_BUNDLE_SIZE).await {
        Ok(bytes) => bytes,
        Err(error) if error.code == ApiErrorCode::PayloadTooLarge => {
            // Oversized uploads are failed replacements. Clear first so they
            // have the same inbox semantics as malformed bundle uploads.
            bundle_store_for_request(env, request)
                .map_err(internal_error)?
                .clear(crate::request_timestamp(request, env).value())
                .await
                .map_err(internal_error)?;
            return Err(error);
        }
        Err(error) => return Err(error),
    };
    let store = bundle_store_for_request(env, request).map_err(internal_error)?;
    let published = store
        .push(bytes, crate::request_timestamp(request, env).value())
        .await
        .map_err(map_bundle_error)?;
    published_bundle_response(published)
}

async fn clear_bundle(mut request: Request, context: RouteContext<()>) -> Result<Response> {
    finish(clear_bundle_inner(&mut request, &context.env).await)
}

async fn clear_bundle_inner(request: &mut Request, env: &Env) -> ApiResult<EmptyInboxResponse> {
    let sender = authenticate_sender(request, env).await?;
    require_capability(sender, prs_sync_protocol::SenderCapability::ClearInbox)?;
    let store = bundle_store_for_request(env, request).map_err(internal_error)?;
    let revision = store
        .clear(crate::request_timestamp(request, env).value())
        .await
        .map_err(internal_error)?;
    Ok(EmptyInboxResponse {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        revision,
        state: "empty",
    })
}

async fn list_credentials(request: Request, context: RouteContext<()>) -> Result<Response> {
    finish(list_credentials_inner(&request, &context.env).await)
}

async fn list_credentials_inner(request: &Request, env: &Env) -> ApiResult<CredentialListResponse> {
    let sender = authenticate_sender(request, env).await?;
    require_capability(
        sender,
        prs_sync_protocol::SenderCapability::ManageCredentials,
    )?;
    let credentials = authorization_service(env)
        .map_err(internal_error)?
        .list_sender_credentials()
        .await
        .map_err(map_authorization_error)?;
    Ok(CredentialListResponse {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        credentials,
    })
}

async fn revoke_credentials(mut request: Request, context: RouteContext<()>) -> Result<Response> {
    finish(revoke_credentials_inner(&mut request, &context.env, None).await)
}

async fn revoke_named_credential(
    mut request: Request,
    context: RouteContext<()>,
) -> Result<Response> {
    let name = context.param("name").cloned();
    finish(revoke_credentials_inner(&mut request, &context.env, name).await)
}

async fn revoke_credentials_inner(
    request: &mut Request,
    env: &Env,
    path_name: Option<String>,
) -> ApiResult<CredentialRevokeResponse> {
    let sender = authenticate_sender(request, env).await?;
    require_capability(
        sender,
        prs_sync_protocol::SenderCapability::ManageCredentials,
    )?;
    let name = match path_name {
        Some(name) => SenderCredentialName::new(name)
            .map_err(|_| ApiFailure::invalid("invalid credential name"))?,
        None => {
            let body: CredentialRevokeBody =
                read_json_body(request, MAX_CREDENTIAL_MANAGEMENT_BODY_SIZE).await?;
            require_version(body.protocol_version)?;
            body.name
        }
    };
    let revoked = authorization_service(env)
        .map_err(internal_error)?
        .revoke_sender_credential(&name, crate::request_timestamp(request, env))
        .await
        .map_err(map_authorization_error)?;
    if !revoked {
        return Err(ApiFailure::new(
            404,
            ApiErrorCode::NotFound,
            "active sender credential was not found",
        ));
    }
    Ok(CredentialRevokeResponse {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        name,
        revoked,
    })
}

async fn reader_manifest(request: Request, context: RouteContext<()>) -> Result<Response> {
    match reader_manifest_inner(&request, &context.env).await {
        Ok((response, revision, etag)) => match manifest_response(response, revision, etag) {
            Ok(response) => Ok(response),
            Err(error) => error_response(error),
        },
        Err(error) => error_response(error),
    }
}

fn manifest_response(
    response: InboxManifestResponse,
    revision: InboxRevision,
    etag: Option<EntityTag>,
) -> ApiResult<Response> {
    let mut builder = ResponseBuilder::new().with_status(200);
    builder = builder
        .with_header("X-PRSync-Revision", &revision.value().to_string())
        .map_err(internal_error)?;
    if let Some(etag) = etag {
        builder = builder
            .with_header("ETag", &quoted_etag(&etag))
            .map_err(internal_error)?;
    }
    builder.from_json(&response).map_err(internal_error)
}

async fn reader_manifest_inner(
    request: &Request,
    env: &Env,
) -> ApiResult<(InboxManifestResponse, InboxRevision, Option<EntityTag>)> {
    let reader = authenticate_reader(request, env).await?;
    require_capability(reader, prs_sync_protocol::ReaderCapability::ReadManifest)?;
    let current = bundle_store_for_request(env, request)
        .map_err(internal_error)?
        .current()
        .await
        .map_err(internal_error)?;
    let if_revision = header_revision(request, "If-Revision")?;
    let if_none_match = header_etag(request)?;
    let state = match (current.manifest, current.etag) {
        (None, None) => InboxManifestState::Empty {
            revision: current.revision,
        },
        (Some(_manifest), Some(etag))
            if if_revision == Some(current.revision) || if_none_match.as_ref() == Some(&etag) =>
        {
            InboxManifestState::NotModified {
                revision: current.revision,
                etag,
            }
        }
        (Some(manifest), Some(etag)) => InboxManifestState::Current {
            revision: current.revision,
            etag,
            manifest,
        },
        _ => return Err(ApiFailure::internal()),
    };
    let etag = match &state {
        InboxManifestState::Current { etag, .. } | InboxManifestState::NotModified { etag, .. } => {
            Some(etag.clone())
        }
        InboxManifestState::Empty { .. } => None,
    };
    Ok((
        InboxManifestResponse {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            state,
        },
        current.revision,
        etag,
    ))
}

async fn reader_bundle(request: Request, context: RouteContext<()>) -> Result<Response> {
    match reader_bundle_inner(&request, &context.env).await {
        Ok(response) => Ok(response),
        Err(error) => error_response(error),
    }
}

async fn reader_bundle_inner(request: &Request, env: &Env) -> ApiResult<Response> {
    let reader = authenticate_reader(request, env).await?;
    require_capability(reader, prs_sync_protocol::ReaderCapability::DownloadBundle)?;
    let store = bundle_store_for_request(env, request).map_err(internal_error)?;
    let current = store.current().await.map_err(internal_error)?;
    let etag = current
        .etag
        .as_ref()
        .ok_or_else(|| ApiFailure::new(404, ApiErrorCode::NotFound, "the inbox is empty"))?;
    let object = store
        .current_object(&current)
        .await
        .map_err(internal_error)?
        .ok_or_else(ApiFailure::internal)?;
    let body = object
        .body()
        .ok_or_else(ApiFailure::internal)?
        .response_body()
        .map_err(internal_error)?;
    ResponseBuilder::new()
        .with_status(200)
        .with_header("Content-Type", "application/x-tar")
        .map_err(internal_error)
        .and_then(|builder| {
            builder
                .with_header("ETag", &quoted_etag(etag))
                .map_err(internal_error)
        })
        .map(|builder| builder.body(body))
}

async fn authenticate_sender(
    request: &Request,
    env: &Env,
) -> ApiResult<crate::authorization::SenderAuthorization> {
    let token = bearer_token(request)?;
    authorization_service(env)
        .map_err(internal_error)?
        .authenticate_sender(&token, crate::request_timestamp(request, env))
        .await
        .map_err(map_authorization_error)
}

async fn authenticate_reader(
    request: &Request,
    env: &Env,
) -> ApiResult<crate::authorization::ReaderAuthorization> {
    let token = bearer_token(request)?;
    authorization_service(env)
        .map_err(internal_error)?
        .authenticate_reader(&token, crate::request_timestamp(request, env))
        .await
        .map_err(map_authorization_error)
}

fn bearer_token(request: &Request) -> ApiResult<BearerToken> {
    let value = request
        .headers()
        .get("Authorization")
        .map_err(|_| ApiFailure::unauthorized())?
        .ok_or_else(ApiFailure::unauthorized)?;
    let token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .filter(|token| !token.is_empty() && !token.chars().any(char::is_whitespace))
        .ok_or_else(ApiFailure::unauthorized)?;
    BearerToken::new(token.to_owned()).map_err(|_| ApiFailure::unauthorized())
}

async fn read_json_body<T: DeserializeOwned>(request: &mut Request, limit: u64) -> ApiResult<T> {
    let bytes = read_limited_body(request, limit).await?;
    serde_json::from_slice(&bytes)
        .map_err(|_| ApiFailure::invalid("request body must be valid JSON"))
}

async fn read_limited_body(request: &mut Request, limit: u64) -> ApiResult<Vec<u8>> {
    let content_length = request
        .headers()
        .get("Content-Length")
        .map_err(|_| ApiFailure::invalid("invalid Content-Length header"))?;
    let mut stream = request
        .stream()
        .map_err(|_| ApiFailure::invalid("request body could not be read"))?;
    if let Err(error) = check_content_length(content_length.as_deref(), limit) {
        drain_body(&mut stream).await;
        return Err(error);
    }

    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ApiFailure::invalid("request body could not be read"))?;
        if let Err(error) = append_body_chunk(&mut body, &chunk, limit) {
            if error.code == ApiErrorCode::PayloadTooLarge {
                // Consume the remainder without retaining it. Workerd requires
                // request bodies to be drained before the response is sent.
                drain_body(&mut stream).await;
            }
            return Err(error);
        }
    }
    Ok(body)
}

async fn drain_body(stream: &mut worker::ByteStream) {
    while stream.next().await.is_some() {}
}

fn check_content_length(value: Option<&str>, limit: u64) -> ApiResult<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let length = value
        .parse::<u64>()
        .map_err(|_| ApiFailure::invalid("Content-Length must be an unsigned integer"))?;
    if length > limit {
        return Err(payload_too_large());
    }
    Ok(())
}

fn append_body_chunk(body: &mut Vec<u8>, chunk: &[u8], limit: u64) -> ApiResult<()> {
    let body_length = u64::try_from(body.len()).map_err(|_| ApiFailure::internal())?;
    let chunk_length = u64::try_from(chunk.len()).map_err(|_| ApiFailure::internal())?;
    let new_length = body_length
        .checked_add(chunk_length)
        .ok_or_else(ApiFailure::internal)?;
    if new_length > limit {
        return Err(payload_too_large());
    }
    body.extend_from_slice(chunk);
    Ok(())
}

fn payload_too_large() -> ApiFailure {
    ApiFailure::new(
        413,
        ApiErrorCode::PayloadTooLarge,
        "request body exceeds the applicable size limit",
    )
}

fn require_capability<T>(authorization: T, capability: impl Capability<T>) -> ApiResult<()>
where
    T: CapabilityTarget,
{
    if capability.allows(&authorization) {
        Ok(())
    } else {
        Err(ApiFailure::new(
            403,
            ApiErrorCode::Forbidden,
            "the bearer token does not grant this operation",
        ))
    }
}

trait Capability<T> {
    fn allows(&self, authorization: &T) -> bool;
}

trait CapabilityTarget {}

impl CapabilityTarget for crate::authorization::SenderAuthorization {}
impl CapabilityTarget for crate::authorization::ReaderAuthorization {}

impl Capability<crate::authorization::SenderAuthorization> for prs_sync_protocol::SenderCapability {
    fn allows(&self, authorization: &crate::authorization::SenderAuthorization) -> bool {
        authorization.allows(*self)
    }
}

impl Capability<crate::authorization::ReaderAuthorization> for prs_sync_protocol::ReaderCapability {
    fn allows(&self, authorization: &crate::authorization::ReaderAuthorization) -> bool {
        authorization.allows(*self)
    }
}

fn authorization_service(env: &Env) -> Result<AuthorizationService> {
    let config = AuthorizationConfig {
        approval_base_url: crate::approval::configured_approval_base_url(env),
        ..AuthorizationConfig::default()
    };
    Ok(AuthorizationService::new(
        env.d1(crate::D1_BINDING)?,
        config,
    ))
}

async fn enforce_creation_rate_limit(
    service: &AuthorizationService,
    request: &Request,
    env: &Env,
) -> ApiResult<()> {
    let client_key = authorization_client_key(request)?;
    match service
        .check_creation_rate_limit(&client_key, crate::request_timestamp(request, env))
        .await
        .map_err(internal_error)?
    {
        RateLimitDecision::Allowed => Ok(()),
        RateLimitDecision::Limited {
            retry_after_seconds,
        } => Err(ApiFailure::rate_limited(retry_after_seconds)),
    }
}

async fn enforce_poll_rate_limit(
    service: &AuthorizationService,
    request: &Request,
    env: &Env,
) -> ApiResult<()> {
    let client_key = authorization_client_key(request)?;
    match service
        .check_poll_rate_limit(&client_key, crate::request_timestamp(request, env))
        .await
        .map_err(internal_error)?
    {
        RateLimitDecision::Allowed => Ok(()),
        RateLimitDecision::Limited {
            retry_after_seconds,
        } => Err(ApiFailure::rate_limited(retry_after_seconds)),
    }
}

fn authorization_client_key(request: &Request) -> ApiResult<String> {
    let client_key = request
        .headers()
        .get("CF-Connecting-IP")
        .map_err(|_| ApiFailure::invalid("invalid client identity header"))?
        .filter(|value| {
            !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_whitespace)
        })
        .unwrap_or_else(|| "anonymous".to_owned());
    Ok(client_key)
}

fn bundle_store_for_request(request_env: &Env, request: &Request) -> Result<BundleStore> {
    #[cfg(feature = "local-test")]
    {
        crate::bundle_store_with_fault(
            request_env,
            crate::test_support::fault_for_request(request, request_env)?,
        )
    }
    #[cfg(not(feature = "local-test"))]
    {
        let _ = request;
        crate::bundle_store(request_env)
    }
}

fn parse_request_id(value: Option<&String>) -> ApiResult<AuthorizationRequestId> {
    value
        .ok_or_else(|| ApiFailure::invalid("authorization request ID is missing"))
        .and_then(|value| {
            AuthorizationRequestId::new(value.clone())
                .map_err(|_| ApiFailure::invalid("authorization request ID is invalid"))
        })
}

fn require_version(version: ProtocolVersion) -> ApiResult<()> {
    version
        .require_compatible(CURRENT_PROTOCOL_VERSION)
        .map_err(|_| {
            ApiFailure::new(
                400,
                ApiErrorCode::UnsupportedVersion,
                "the protocol version is not supported",
            )
        })
}

fn header_revision(request: &Request, name: &str) -> ApiResult<Option<InboxRevision>> {
    let value = request
        .headers()
        .get(name)
        .map_err(|_| ApiFailure::invalid("invalid revision header"))?;
    value
        .map(|value| {
            value
                .parse::<u64>()
                .map(InboxRevision::new)
                .map_err(|_| ApiFailure::invalid("revision header must be an unsigned integer"))
        })
        .transpose()
}

fn header_etag(request: &Request) -> ApiResult<Option<EntityTag>> {
    let value = request
        .headers()
        .get("If-None-Match")
        .map_err(|_| ApiFailure::invalid("invalid ETag header"))?;
    value
        .map(|value| {
            let value = value.trim();
            let value = value
                .strip_prefix('\"')
                .and_then(|value| value.strip_suffix('\"'))
                .unwrap_or(value);
            EntityTag::new(value.to_owned())
                .map_err(|_| ApiFailure::invalid("ETag header is invalid"))
        })
        .transpose()
}

fn quoted_etag(etag: &EntityTag) -> String {
    format!("\"{}\"", etag.as_str())
}

fn published_bundle_response(published: PublishedBundle) -> ApiResult<BundleResponse> {
    Ok(BundleResponse {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        revision: published.revision,
        etag: EntityTag::new(published.etag).map_err(|_| ApiFailure::internal())?,
        size_bytes: published.size_bytes,
    })
}

fn internal_error<T>(_error: T) -> ApiFailure {
    ApiFailure::internal()
}

fn map_bundle_error(error: BundlePushError) -> ApiFailure {
    match error {
        BundlePushError::InvalidBundle(_) => ApiFailure::new(
            422,
            ApiErrorCode::InvalidRequest,
            "the bundle is invalid or exceeds the protocol size limit",
        ),
        BundlePushError::Storage(_) => ApiFailure::internal(),
    }
}

fn map_authorization_error(error: AuthorizationError) -> ApiFailure {
    match error.failure() {
        Some(AuthorizationFailure::NotFound) => ApiFailure::new(
            404,
            ApiErrorCode::NotFound,
            "authorization request was not found",
        ),
        Some(AuthorizationFailure::Unauthorized)
        | Some(AuthorizationFailure::InvalidPollingSecret) => ApiFailure::unauthorized(),
        Some(AuthorizationFailure::WrongAuthorizationKind) => {
            ApiFailure::invalid("authorization request kind does not match the operation")
        }
        Some(AuthorizationFailure::Expired) => {
            ApiFailure::new(410, ApiErrorCode::Gone, "authorization request has expired")
        }
        Some(AuthorizationFailure::Denied) => ApiFailure::new(
            409,
            ApiErrorCode::Conflict,
            "authorization request was denied",
        ),
        Some(AuthorizationFailure::AlreadyClaimed) => ApiFailure::new(
            409,
            ApiErrorCode::Conflict,
            "authorization request was already claimed",
        ),
        Some(AuthorizationFailure::InvalidState) => ApiFailure::new(
            409,
            ApiErrorCode::Conflict,
            "authorization request is not in a valid state",
        ),
        Some(AuthorizationFailure::CredentialAlreadyExists) => ApiFailure::new(
            409,
            ApiErrorCode::Conflict,
            "an active sender credential already uses this name",
        ),
        None => ApiFailure::internal(),
    }
}

fn finish<T: Serialize>(result: ApiResult<T>) -> Result<Response> {
    match result {
        Ok(value) => json_response(&value, 200),
        Err(error) => error_response(error),
    }
}

fn json_response<T: Serialize>(value: &T, status: u16) -> Result<Response> {
    Response::from_json(value).map(|response| response.with_status(status))
}

fn error_response(error: ApiFailure) -> Result<Response> {
    let body = ApiError {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        error: ApiErrorBody {
            code: error.code,
            message: error.message,
            request_id: None,
        },
    };
    let mut builder = ResponseBuilder::new().with_status(error.status);
    if let Some(retry_after_seconds) = error.retry_after_seconds {
        builder = builder.with_header("Retry-After", &retry_after_seconds.to_string())?;
    }
    builder.from_json(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_contract_is_versioned_and_separates_capabilities() {
        for path in ["/api/v1/authorization", "/api/v1/sender", "/api/v1/reader"] {
            assert!(path.starts_with("/api/v1/"));
        }
        assert_ne!("/api/v1/sender/bundle", "/api/v1/reader/bundle");
    }

    #[test]
    fn bearer_token_parser_requires_the_bearer_scheme() {
        assert!(BearerToken::new("abc").is_ok());
        assert!("Basic abc".strip_prefix("Bearer ").is_none());
        assert!("Bearer abc def".chars().any(char::is_whitespace));
    }

    #[test]
    fn etag_headers_are_normalized_to_protocol_values() {
        let raw = "\"etag-1\"";
        let value = raw
            .strip_prefix('\"')
            .and_then(|value| value.strip_suffix('\"'))
            .unwrap_or(raw);
        assert_eq!(EntityTag::new(value).unwrap().as_str(), "etag-1");
        assert_eq!(quoted_etag(&EntityTag::new("etag-1").unwrap()), raw);
    }

    #[test]
    fn sender_push_response_contains_metadata_but_not_manifest_content() {
        let response = BundleResponse {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            revision: InboxRevision::new(3),
            etag: EntityTag::new("etag-1").unwrap(),
            size_bytes: 128,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("size_bytes"));
        assert!(!json.contains("manifest"));
    }

    #[test]
    fn sender_push_response_uses_committed_publication_metadata() {
        let response = published_bundle_response(PublishedBundle {
            bundle_id: "bundle-1".into(),
            object_key: "bundles/candidates/bundle-1.tar".into(),
            revision: InboxRevision::new(9),
            etag: "etag-1".into(),
            manifest: prs_sync_protocol::Manifest {
                protocol_version: CURRENT_PROTOCOL_VERSION,
                bundle_format_version: prs_sync_protocol::CURRENT_BUNDLE_FORMAT_VERSION,
                entry_point: prs_sync_protocol::BundlePath::new("index.md").unwrap(),
                files: Vec::new(),
            },
            size_bytes: 256,
        })
        .unwrap();

        assert_eq!(response.revision, InboxRevision::new(9));
        assert_eq!(response.etag.as_str(), "etag-1");
        assert_eq!(response.size_bytes, 256);
    }

    #[test]
    fn duplicate_credentials_are_conflicts_in_the_api_error_mapping() {
        let error = map_authorization_error(AuthorizationError::Authorization(
            AuthorizationFailure::CredentialAlreadyExists,
        ));
        assert_eq!(error.status, 409);
        assert_eq!(error.code, ApiErrorCode::Conflict);
    }

    #[test]
    fn invalid_bundle_errors_are_unprocessable_requests() {
        let error = map_bundle_error(BundlePushError::InvalidBundle(worker::Error::RustError(
            "invalid archive".into(),
        )));
        assert_eq!(error.status, 422);
        assert_eq!(error.code, ApiErrorCode::InvalidRequest);
    }

    #[test]
    fn body_limits_accept_the_boundary_and_reject_the_next_byte() {
        let mut body = Vec::new();
        assert!(append_body_chunk(&mut body, b"1234", 4).is_ok());
        let error = append_body_chunk(&mut body, b"5", 4).unwrap_err();
        assert_eq!(error.status, 413);
        assert_eq!(error.code, ApiErrorCode::PayloadTooLarge);
        assert_eq!(body, b"1234");
    }

    #[test]
    fn content_length_is_an_early_check_only() {
        assert!(check_content_length(Some("4"), 4).is_ok());
        let mut body = Vec::new();
        assert!(check_content_length(Some("0"), 4).is_ok());
        assert_eq!(
            append_body_chunk(&mut body, b"12345", 4).unwrap_err().code,
            ApiErrorCode::PayloadTooLarge
        );
        assert_eq!(
            check_content_length(Some("5"), 4).unwrap_err().code,
            ApiErrorCode::PayloadTooLarge
        );
        assert_eq!(
            check_content_length(Some("not-a-number"), 4)
                .unwrap_err()
                .code,
            ApiErrorCode::InvalidRequest
        );
    }

    #[test]
    fn storage_errors_are_internal_server_errors() {
        let error = map_bundle_error(BundlePushError::Storage(worker::Error::RustError(
            "database unavailable".into(),
        )));
        assert_eq!(error.status, 500);
        assert_eq!(error.code, ApiErrorCode::Internal);
    }
}
