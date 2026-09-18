//! Human-facing approval routes and the deliberately small approval page.

use crate::authorization::{
    ApprovalRequestDetails, AuthorizationError, AuthorizationFailure, AuthorizationService,
};
use crate::identity::PrincipalBoundary;
use prs_sync_protocol::{AuthorizationKind, AuthorizationRequestId, AuthorizationState, Timestamp};
use worker::{Date, Env, Request, Response, Result};

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

pub async fn show(request: Request, env: Env, request_id: Option<String>) -> Result<Response> {
    render_route(request, env, request_id, None).await
}

pub async fn approve(request: Request, env: Env, request_id: Option<String>) -> Result<Response> {
    action_route(request, env, request_id, true).await
}

pub async fn deny(request: Request, env: Env, request_id: Option<String>) -> Result<Response> {
    action_route(request, env, request_id, false).await
}

async fn action_route(
    request: Request,
    env: Env,
    request_id: Option<String>,
    approve: bool,
) -> Result<Response> {
    let request_id = parse_request_id(request_id)?;
    let service = authorization_service(&env)?;
    let owner = match authenticate_owner(&request, &env, &service).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let current_time = now();
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

    let details = match service.approval_details(&request_id, now()).await {
        Ok(details) => details,
        Err(error) => return authorization_error_response(error),
    };
    let message = if approve {
        "The request was approved. The waiting client can now claim its scoped credential."
    } else {
        "The request was denied. The waiting client will not receive a credential."
    };
    render(details, Some(message))
}

async fn render_route(
    request: Request,
    env: Env,
    request_id: Option<String>,
    message: Option<&'static str>,
) -> Result<Response> {
    let request_id = parse_request_id(request_id)?;
    let service = authorization_service(&env)?;
    if let Err(response) = authenticate_owner(&request, &env, &service).await {
        return response;
    }
    let details = match service.approval_details(&request_id, now()).await {
        Ok(details) => details,
        Err(error) => return authorization_error_response(error),
    };
    render(details, message)
}

async fn authenticate_owner(
    request: &Request,
    env: &Env,
    service: &AuthorizationService,
) -> std::result::Result<crate::authorization::OwnerApprovalCapability, Result<Response>> {
    let boundary =
        PrincipalBoundary::from_env(env).map_err(|error| -> Result<Response> { Err(error) })?;
    let principal = boundary
        .principal(request)
        .map_err(|_| forbidden_response())?;
    service
        .authenticate_owner(&principal)
        .await
        .map_err(authorization_error_response)
}

fn authorization_service(env: &Env) -> Result<AuthorizationService> {
    let mut config = crate::authorization::AuthorizationConfig::default();
    if let Ok(base_url) = env.var("PRS_APPROVAL_BASE_URL") {
        config.approval_base_url = base_url.to_string();
    }
    Ok(AuthorizationService::new(
        env.d1(crate::D1_BINDING)?,
        config,
    ))
}

fn parse_request_id(value: Option<String>) -> Result<AuthorizationRequestId> {
    value
        .ok_or_else(|| worker::Error::RustError("authorization request ID is missing".into()))
        .and_then(|value| {
            AuthorizationRequestId::new(value)
                .map_err(|error| worker::Error::RustError(error.to_string()))
        })
}

fn now() -> Timestamp {
    Timestamp::new(Date::now().as_millis() / 1_000)
}

fn render(details: ApprovalRequestDetails, message: Option<&str>) -> Result<Response> {
    let html = render_html(details, message);
    worker::ResponseBuilder::new()
        .with_header("Cache-Control", "no-store")?
        .with_header(
            "Content-Security-Policy",
            "default-src 'none'; style-src 'unsafe-inline'",
        )?
        .from_html(html)
}

fn render_html(details: ApprovalRequestDetails, message: Option<&str>) -> String {
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
        format!(
            r#"<div class="actions">
                <form method="post" action="/a/{request_id}/approve">
                    <button class="approve" type="submit">Approve request</button>
                </form>
                <form method="post" action="/a/{request_id}/deny">
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
    Response::error("approval requires the configured owner identity", 403)
}

fn authorization_error_response(error: AuthorizationError) -> Result<Response> {
    if let Some(failure) = error.failure() {
        let status = match failure {
            AuthorizationFailure::NotFound => 404,
            AuthorizationFailure::Unauthorized => 403,
            AuthorizationFailure::Expired => 410,
            AuthorizationFailure::Denied
            | AuthorizationFailure::AlreadyClaimed
            | AuthorizationFailure::InvalidState => 409,
            AuthorizationFailure::WrongAuthorizationKind
            | AuthorizationFailure::InvalidPollingSecret => 400,
        };
        return Response::error(failure.to_string(), status);
    }
    Err(error.into_worker())
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
        let body = render_html(details(AuthorizationKind::Sender), None);
        assert!(body.contains("sender"));
        assert!(body.contains("laptop&lt;&amp;"));
        assert!(body.contains("/a/auth-test/approve"));
        assert!(body.contains("/a/auth-test/deny"));
        assert!(!body.contains("polling_secret"));
        assert!(!body.contains("bearer_token"));
    }

    #[test]
    fn reader_page_does_not_invent_sender_context() {
        let body = render_html(details(AuthorizationKind::Reader), None);
        assert!(body.contains("reader"));
        assert!(body.contains("Not applicable"));
        assert!(body.contains("Approve request"));
        assert!(body.contains("Deny request"));
    }
}
