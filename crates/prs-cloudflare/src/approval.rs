//! Human-facing approval routes and the deliberately small approval page.

use crate::authorization::{
    ApprovalRequestDetails, AuthorizationError, AuthorizationFailure, AuthorizationService,
};
use crate::identity::AccessContext;
#[cfg(test)]
use prs_sync_protocol::Timestamp;
use prs_sync_protocol::{AuthorizationKind, AuthorizationRequestId, AuthorizationState};
use sha2::{Digest, Sha256};
use worker::js_sys::{self, Function, Uint8Array};
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::{Env, Request, Response, Result};

const APPROVAL_BASE_URL_ENV: &str = "PRS_APPROVAL_BASE_URL";
const CSRF_SECRET_ENV: &str = "PRS_CSRF_SECRET";
const DEFAULT_APPROVAL_BASE_URL: &str = "https://prs-reader.dstoc.workers.dev";
const LOCAL_ENVIRONMENT: &str = "local";
const CSRF_FIELD: &str = "csrf_token";
const CSRF_TOKEN_VERSION: &str = "v1";
const CSRF_NONCE_BYTES: usize = 32;
const CSRF_MIN_SECRET_BYTES: usize = 32;
const ORIGIN_HEADER: &str = "Origin";
const APPROVAL_CSP: &str =
    "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'; form-action 'self'";
const APPROVAL_PAGE_STYLE: &str = r#"
    :root { color-scheme: light; font-family: system-ui, sans-serif; }
    body { margin: 0; background: #f4f5f7; color: #17202a; }
    main { box-sizing: border-box; max-width: 38rem; margin: 4rem auto; padding: 2rem;
           background: white; border: 1px solid #d6d9de; border-radius: .75rem;
           box-shadow: 0 .5rem 2rem #17202a18; }
    h1 { margin-top: 0; font-size: 1.5rem; }
    dl { display: grid; grid-template-columns: 10rem 1fr; gap: .75rem 1rem; }
    dt { color: #53606d; }
    dd { margin: 0; overflow-wrap: anywhere; }
    .notice { padding: .75rem 1rem; border-radius: .5rem; background: #edf4ff; }
    .actions { display: flex; gap: .75rem; margin-top: 2rem; }
    button { border: 0; border-radius: .4rem; cursor: pointer; font: inherit;
             padding: .75rem 1.25rem; }
    .approve { background: #176b3a; color: white; }
    .deny { background: #a4262c; color: white; }
    .muted { color: #53606d; font-size: .9rem; }
"#;

pub async fn show(
    request: Request,
    env: Env,
    request_id: Option<String>,
    access: Option<AccessContext>,
) -> Result<Response> {
    match approval_host_allowed(&request, &env) {
        Ok(true) => {}
        Ok(false) => return wrong_approval_host_response(),
        Err(_) => return approval_unavailable_response(),
    }
    render_route(request, env, request_id, None, access).await
}

pub async fn approve(
    request: Request,
    env: Env,
    request_id: Option<String>,
    access: Option<AccessContext>,
) -> Result<Response> {
    match approval_host_allowed(&request, &env) {
        Ok(true) => {}
        Ok(false) => return wrong_approval_host_response(),
        Err(_) => return approval_unavailable_response(),
    }
    action_route(request, env, request_id, true, access).await
}

pub async fn deny(
    request: Request,
    env: Env,
    request_id: Option<String>,
    access: Option<AccessContext>,
) -> Result<Response> {
    match approval_host_allowed(&request, &env) {
        Ok(true) => {}
        Ok(false) => return wrong_approval_host_response(),
        Err(_) => return approval_unavailable_response(),
    }
    action_route(request, env, request_id, false, access).await
}

async fn action_route(
    mut request: Request,
    env: Env,
    request_id: Option<String>,
    approve: bool,
    access: Option<AccessContext>,
) -> Result<Response> {
    let request_id = match parse_request_id(request_id) {
        Ok(request_id) => request_id,
        Err(_) => return invalid_request_response(),
    };
    let access = match access.as_ref() {
        Some(access) => access,
        None => return forbidden_response(),
    };
    if !same_origin(&request)? {
        return csrf_failure_response();
    }
    let submitted_token = request
        .form_data()
        .await
        .ok()
        .and_then(|form| form.get_field(CSRF_FIELD));
    let valid_token = match submitted_token
        .as_deref()
        .map(|token| validate_csrf_token(&env, access, &request_id, token))
        .transpose()
    {
        Ok(valid_token) => valid_token.unwrap_or(false),
        Err(_) => return approval_unavailable_response(),
    };
    if !valid_token {
        return csrf_failure_response();
    }

    let service = match authorization_service(&env) {
        Ok(service) => service,
        Err(_) => return approval_unavailable_response(),
    };
    let owner = match approval_capability(Some(access)) {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let current_time = crate::request_timestamp(&request, &env);
    let details = match service.approval_details(&request_id, current_time).await {
        Ok(details) => details,
        Err(error) => return authorization_error_response(error),
    };
    let result = match (details.kind, approve) {
        (AuthorizationKind::Sender, true) => {
            service
                .approve_sender(owner, &request_id, current_time)
                .await
        }
        (AuthorizationKind::Sender, false) => {
            service.deny_sender(owner, &request_id, current_time).await
        }
        (AuthorizationKind::Reader, true) => {
            service
                .approve_reader(owner, &request_id, current_time)
                .await
        }
        (AuthorizationKind::Reader, false) => {
            service.deny_reader(owner, &request_id, current_time).await
        }
    };
    if let Err(error) = result {
        return authorization_error_response(error);
    }

    let details = match service
        .approval_details(&request_id, crate::request_timestamp(&request, &env))
        .await
    {
        Ok(details) => details,
        Err(error) => return authorization_error_response(error),
    };
    let message = if approve {
        "The request was approved. The waiting client can now claim its scoped credential."
    } else {
        "The request was denied. The waiting client will not receive a credential."
    };
    render_safely(&env, access, details, Some(message))
}

async fn render_route(
    _request: Request,
    env: Env,
    request_id: Option<String>,
    message: Option<&'static str>,
    access: Option<AccessContext>,
) -> Result<Response> {
    let request_id = match parse_request_id(request_id) {
        Ok(request_id) => request_id,
        Err(_) => return invalid_request_response(),
    };
    let service = match authorization_service(&env) {
        Ok(service) => service,
        Err(_) => return approval_unavailable_response(),
    };
    let access = match access.as_ref() {
        Some(access) => access,
        None => return forbidden_response(),
    };
    if let Err(response) = approval_capability(Some(access)) {
        return response;
    }
    let details = match service
        .approval_details(&request_id, crate::request_timestamp(&_request, &env))
        .await
    {
        Ok(details) => details,
        Err(error) => return authorization_error_response(error),
    };
    render_safely(&env, access, details, message)
}

fn approval_capability(
    access: Option<&AccessContext>,
) -> std::result::Result<crate::authorization::OwnerApprovalCapability, Result<Response>> {
    if access_is_authenticated(access) {
        Ok(crate::authorization::OwnerApprovalCapability::new())
    } else {
        Err(forbidden_response())
    }
}

fn access_is_authenticated(access: Option<&AccessContext>) -> bool {
    access.is_some()
}

fn authorization_service(env: &Env) -> Result<AuthorizationService> {
    let config = crate::authorization::AuthorizationConfig {
        approval_base_url: configured_approval_base_url(env),
        ..crate::authorization::AuthorizationConfig::default()
    };
    Ok(AuthorizationService::new(
        env.d1(crate::D1_BINDING)?,
        config,
    ))
}

pub(crate) fn configured_approval_base_url(env: &Env) -> String {
    env.var(APPROVAL_BASE_URL_ENV)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| DEFAULT_APPROVAL_BASE_URL.to_owned())
}

/// Return whether the production approval page has its CSRF secret binding.
///
/// The value is never returned or logged. Readiness uses this check to stop a
/// release before it can advertise an approval surface that cannot render.
pub(crate) fn csrf_secret_configured(env: &Env) -> bool {
    env.var(CSRF_SECRET_ENV)
        .ok()
        .map(|value| value.to_string())
        .is_some_and(|secret| secret.as_bytes().len() >= CSRF_MIN_SECRET_BYTES)
}

fn approval_host_allowed(request: &Request, env: &Env) -> Result<bool> {
    let environment = env
        .var("PRS_ENVIRONMENT")
        .map(|value| value.to_string())
        .unwrap_or_default();
    let request_url = request.url()?;
    approval_host_matches(
        &configured_approval_base_url(env),
        request_url.host_str(),
        &environment,
    )
}

fn approval_host_matches(
    configured_base_url: &str,
    request_host: Option<&str>,
    environment: &str,
) -> Result<bool> {
    let configured_url = worker::Url::parse(configured_base_url)
        .map_err(|error| worker::Error::RustError(error.to_string()))?;
    let configured_host = configured_url
        .host_str()
        .ok_or_else(|| worker::Error::RustError("approval URL has no hostname".into()))?;
    Ok(request_host.is_some_and(|request_host| {
        request_host.eq_ignore_ascii_case(configured_host)
            && (environment == LOCAL_ENVIRONMENT || configured_url.scheme() == "https")
    }))
}

fn parse_request_id(value: Option<String>) -> Result<AuthorizationRequestId> {
    value
        .ok_or_else(|| worker::Error::RustError("authorization request ID is missing".into()))
        .and_then(|value| {
            AuthorizationRequestId::new(value)
                .map_err(|error| worker::Error::RustError(error.to_string()))
        })
}

fn render(
    env: &Env,
    access: &AccessContext,
    details: ApprovalRequestDetails,
    message: Option<&str>,
) -> Result<Response> {
    let csrf_token = (details.state == AuthorizationState::Pending)
        .then(|| create_csrf_token(env, access, &details.request_id))
        .transpose()?;
    let html = render_html(details, message, csrf_token.as_deref());
    worker::ResponseBuilder::new()
        .with_header("Cache-Control", "no-store")?
        .with_header("Content-Security-Policy", APPROVAL_CSP)?
        .with_header("X-Frame-Options", "DENY")?
        .from_html(html)
}

fn render_safely(
    env: &Env,
    access: &AccessContext,
    details: ApprovalRequestDetails,
    message: Option<&str>,
) -> Result<Response> {
    match render(env, access, details, message) {
        Ok(response) => Ok(response),
        Err(_) => approval_unavailable_response(),
    }
}

fn render_html(
    details: ApprovalRequestDetails,
    message: Option<&str>,
    csrf_token: Option<&str>,
) -> String {
    let kind = match details.kind {
        AuthorizationKind::Sender => "sender",
        AuthorizationKind::Reader => "reader",
    };
    let state = state_label(details.state);
    let credential = details
        .credential_name
        .as_ref()
        .map(|name| escape_html(name.as_str()))
        .unwrap_or_else(|| "Not applicable".to_owned());
    let request_id = escape_html(details.request_id.as_str());
    let created_at = details.created_at.value().to_string();
    let expires_at = details.expires_at.value().to_string();
    let action_markup = if details.state == AuthorizationState::Pending {
        let csrf_input = csrf_token
            .map(|token| {
                format!(
                    r#"<input type="hidden" name="{CSRF_FIELD}" value="{}">"#,
                    escape_html(token)
                )
            })
            .unwrap_or_default();
        format!(
            r#"<div class="actions">
                <form method="post" action="/a/{request_id}/approve">
                    {csrf_input}
                    <button class="approve" type="submit">Approve request</button>
                </form>
                <form method="post" action="/a/{request_id}/deny">
                    {csrf_input}
                    <button class="deny" type="submit">Deny request</button>
                </form>
            </div>"#
        )
    } else {
        String::from(
            r#"<p class="muted">This request has reached a terminal state. No further action is available.</p>"#,
        )
    };
    let notice = message
        .map(|message| format!(r#"<p class="notice">{}</p>"#, escape_html(message)))
        .unwrap_or_default();
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>PRSync authorization</title>
<style>{APPROVAL_PAGE_STYLE}</style>
</head>
<body>
<main>
<h1>PRSync authorization request</h1>
{notice}
<p>Review this request before you approve or deny it.</p>
<dl>
<dt>Request type</dt><dd>{kind}</dd>
<dt>Sender credential</dt><dd>{credential}</dd>
<dt>Request ID</dt><dd>{request_id}</dd>
<dt>Created at</dt><dd>{created_at} (Unix time)</dd>
<dt>Expires at</dt><dd>{expires_at} (Unix time)</dd>
<dt>Current state</dt><dd>{state}</dd>
</dl>
{action_markup}
<p class="muted">The approval page never receives the polling secret or the resulting bearer credential.</p>
</main>
</body>
</html>"#
    )
}

fn state_label(state: AuthorizationState) -> &'static str {
    match state {
        AuthorizationState::Pending => "pending",
        AuthorizationState::Approved => "approved",
        AuthorizationState::Denied => "denied",
        AuthorizationState::Expired => "expired",
        AuthorizationState::Claimed => "claimed",
    }
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn forbidden_response() -> Result<Response> {
    secure_approval_response(Response::error(
        "approval requires an authenticated Access context",
        403,
    )?)
}

fn csrf_failure_response() -> Result<Response> {
    secure_approval_response(Response::error(
        "approval action failed CSRF validation",
        403,
    )?)
}

fn secure_approval_response(mut response: Response) -> Result<Response> {
    response.headers_mut().set("Cache-Control", "no-store")?;
    response
        .headers_mut()
        .set("Content-Security-Policy", APPROVAL_CSP)?;
    response.headers_mut().set("X-Frame-Options", "DENY")?;
    Ok(response)
}

fn same_origin(request: &Request) -> Result<bool> {
    let Some(origin) = request.headers().get(ORIGIN_HEADER)? else {
        return Ok(false);
    };
    let Ok(origin_url) = worker::Url::parse(&origin) else {
        return Ok(false);
    };
    if !origin_url.username().is_empty()
        || origin_url.password().is_some()
        || origin_url.path() != "/"
        || origin_url.query().is_some()
        || origin_url.fragment().is_some()
    {
        return Ok(false);
    }
    Ok(request.url()?.origin() == origin_url.origin())
}

fn create_csrf_token(
    env: &Env,
    access: &AccessContext,
    request_id: &AuthorizationRequestId,
) -> Result<String> {
    let secret = csrf_secret(env)?;
    let nonce = hex_encode(&random_bytes::<CSRF_NONCE_BYTES>()?);
    let signature = csrf_signature(&secret, access, request_id, &nonce);
    Ok(format!("{CSRF_TOKEN_VERSION}.{nonce}.{signature}"))
}

fn validate_csrf_token(
    env: &Env,
    access: &AccessContext,
    request_id: &AuthorizationRequestId,
    token: &str,
) -> Result<bool> {
    let mut parts = token.split('.');
    let (Some(version), Some(nonce), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Ok(false);
    };
    if version != CSRF_TOKEN_VERSION
        || nonce.len() != CSRF_NONCE_BYTES * 2
        || signature.len() != Sha256::output_size() * 2
        || !is_lowercase_hex(nonce)
        || !is_lowercase_hex(signature)
    {
        return Ok(false);
    }
    let expected = csrf_signature(&csrf_secret(env)?, access, request_id, nonce);
    Ok(constant_time_eq(signature.as_bytes(), expected.as_bytes()))
}

fn csrf_secret(env: &Env) -> Result<String> {
    let secret = env.var(CSRF_SECRET_ENV).ok().map(|value| value.to_string());
    checked_csrf_secret(secret.as_deref())
}

fn checked_csrf_secret(secret: Option<&str>) -> Result<String> {
    let Some(secret) = secret else {
        return Err(worker::Error::RustError(
            "approval CSRF configuration is unavailable".to_owned(),
        ));
    };
    if secret.as_bytes().len() < CSRF_MIN_SECRET_BYTES {
        return Err(worker::Error::RustError(
            "approval CSRF configuration is unavailable".to_owned(),
        ));
    }
    Ok(secret.to_owned())
}

fn csrf_signature(
    secret: &str,
    access: &AccessContext,
    request_id: &AuthorizationRequestId,
    nonce: &str,
) -> String {
    let mut key = secret.as_bytes().to_vec();
    if key.len() > 64 {
        key = Sha256::digest(&key).to_vec();
    }
    key.resize(64, 0);

    let mut inner_pad = [0x36; 64];
    let mut outer_pad = [0x5c; 64];
    for (index, byte) in key.iter().enumerate() {
        inner_pad[index] ^= byte;
        outer_pad[index] ^= byte;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    update_csrf_field(&mut inner, access.csrf_binding());
    update_csrf_field(&mut inner, request_id.as_str());
    update_csrf_field(&mut inner, nonce);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    hex_encode(&outer.finalize())
}

fn update_csrf_field(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

fn is_lowercase_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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

fn wrong_approval_host_response() -> Result<Response> {
    secure_approval_response(Response::error(
        "approval routes are not available on this hostname",
        404,
    )?)
}

fn invalid_request_response() -> Result<Response> {
    secure_approval_response(Response::error("approval request was not found", 404)?)
}

fn approval_unavailable_response() -> Result<Response> {
    worker::console_error!("PRSync approval request failed unexpectedly");
    secure_approval_response(Response::error(
        "approval service is temporarily unavailable",
        503,
    )?)
}

fn authorization_error_response(error: AuthorizationError) -> Result<Response> {
    if let Some(failure) = error.failure() {
        let status = match failure {
            AuthorizationFailure::NotFound => 404,
            AuthorizationFailure::CredentialAlreadyExists => 409,
            AuthorizationFailure::Unauthorized => 403,
            AuthorizationFailure::Expired => 410,
            AuthorizationFailure::Denied
            | AuthorizationFailure::AlreadyClaimed
            | AuthorizationFailure::InvalidState => 409,
            AuthorizationFailure::WrongAuthorizationKind
            | AuthorizationFailure::InvalidPollingSecret => 400,
        };
        return secure_approval_response(Response::error(failure.to_string(), status)?);
    }
    approval_unavailable_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use prs_sync_protocol::{AuthorizationRequestId, SenderCredentialName};

    fn details(kind: AuthorizationKind) -> ApprovalRequestDetails {
        ApprovalRequestDetails {
            request_id: AuthorizationRequestId::new("auth-test").unwrap(),
            kind,
            credential_name: (kind == AuthorizationKind::Sender)
                .then(|| SenderCredentialName::new("laptop<&").unwrap()),
            approval_url: "https://reader.example/a/auth-test".to_owned(),
            created_at: Timestamp::new(100),
            expires_at: Timestamp::new(200),
            state: AuthorizationState::Pending,
        }
    }

    #[test]
    fn sender_page_contains_context_and_explicit_actions_without_secrets() {
        let body = render_html(details(AuthorizationKind::Sender), None, Some("csrf-token"));
        assert!(body.contains("sender"));
        assert!(body.contains("laptop&lt;&amp;"));
        assert!(body.contains("/a/auth-test/approve"));
        assert!(body.contains("/a/auth-test/deny"));
        assert!(body.contains("name=\"csrf_token\""));
        assert!(!body.contains("polling_secret"));
        assert!(!body.contains("bearer_token"));
    }

    #[test]
    fn reader_page_does_not_invent_sender_context() {
        let body = render_html(details(AuthorizationKind::Reader), None, Some("csrf-token"));
        assert!(body.contains("reader"));
        assert!(body.contains("Not applicable"));
        assert!(body.contains("Approve request"));
        assert!(body.contains("Deny request"));
    }

    #[test]
    fn approval_routes_accept_only_the_configured_human_host() {
        assert!(approval_host_matches(
            "https://prs-reader.dstoc.workers.dev",
            Some("prs-reader.dstoc.workers.dev"),
            "production",
        )
        .unwrap());
        assert!(!approval_host_matches(
            "https://prs-reader.dstoc.workers.dev",
            Some("reader.example.com"),
            "production",
        )
        .unwrap());
        assert!(approval_host_matches("http://127.0.0.1", Some("127.0.0.1"), "local",).unwrap());
        assert!(!approval_host_matches("http://127.0.0.1", Some("127.0.0.2"), "local",).unwrap());
    }

    #[test]
    fn approval_requires_an_authenticated_access_context() {
        assert!(!access_is_authenticated(None));
    }

    #[test]
    fn csrf_secret_requires_a_configured_minimum_length() {
        assert!(checked_csrf_secret(None).is_err());
        assert!(checked_csrf_secret(Some("too-short")).is_err());
        assert_eq!(
            checked_csrf_secret(Some("a sufficiently long test secret for csrf")).unwrap(),
            "a sufficiently long test secret for csrf"
        );
    }

    #[test]
    fn csrf_tokens_bind_the_request_and_access_context() {
        let access = AccessContext::for_test("access-audience|owner");
        let request_id = AuthorizationRequestId::new("auth-test").unwrap();
        let nonce = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let signature = csrf_signature(
            "a sufficiently long test secret for csrf",
            &access,
            &request_id,
            nonce,
        );
        let token = format!("v1.{nonce}.{signature}");

        assert!(validate_csrf_token_parts(
            "a sufficiently long test secret for csrf",
            &access,
            &request_id,
            &token,
        ));
        assert!(!validate_csrf_token_parts(
            "a sufficiently long test secret for csrf",
            &access,
            &AuthorizationRequestId::new("other-request").unwrap(),
            &token,
        ));
        assert!(!validate_csrf_token_parts(
            "a sufficiently long test secret for csrf",
            &AccessContext::for_test("access-audience|other-owner"),
            &request_id,
            &token,
        ));
    }

    fn validate_csrf_token_parts(
        secret: &str,
        access: &AccessContext,
        request_id: &AuthorizationRequestId,
        token: &str,
    ) -> bool {
        let mut parts = token.split('.');
        let (Some(version), Some(nonce), Some(signature), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        version == CSRF_TOKEN_VERSION
            && constant_time_eq(
                signature.as_bytes(),
                csrf_signature(secret, access, request_id, nonce).as_bytes(),
            )
    }
}
