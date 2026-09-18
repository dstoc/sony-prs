//! The PRSync Cloudflare Worker entry point.
//!
//! The Worker owns all D1 and R2 access. The protocol crate remains a
//! Cloudflare-independent collection of wire types.

mod api;
mod approval;
mod authorization;
mod identity;

pub use authorization::{
    ApprovalRequestDetails, AuthenticatedCapability, AuthorizationConfig, AuthorizationError,
    AuthorizationFailure, AuthorizationResult, AuthorizationService, OwnerApprovalCapability,
    PendingPollingCapability, ReaderAuthorization, SenderAuthorization,
};
pub use identity::{PrincipalBoundary, PrincipalError, TrustedPrincipal};
use worker::*;

mod storage;

pub const D1_BINDING: &str = "DB";
pub const R2_BINDING: &str = "BUNDLES";

pub(crate) fn bundle_store(env: &Env) -> Result<storage::BundleStore> {
    Ok(storage::BundleStore::from_env(
        env.d1(D1_BINDING)?,
        env.bucket(R2_BINDING)?,
    ))
}

#[event(fetch)]
pub async fn fetch(request: Request, env: Env, _context: Context) -> Result<Response> {
    api::register(Router::new())
        .get_async("/a/:request_id", |request, context| async move {
            let request_id = context.param("request_id").cloned();
            approval::show(request, context.env, request_id).await
        })
        .post_async("/a/:request_id/approve", |request, context| async move {
            let request_id = context.param("request_id").cloned();
            approval::approve(request, context.env, request_id).await
        })
        .post_async("/a/:request_id/deny", |request, context| async move {
            let request_id = context.param("request_id").cloned();
            approval::deny(request, context.env, request_id).await
        })
        .run(request, env)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_names_are_stable() {
        assert_eq!(D1_BINDING, "DB");
        assert_eq!(R2_BINDING, "BUNDLES");
    }

    #[test]
    fn approval_routes_are_separate_explicit_actions() {
        assert_ne!("/a/:request_id/approve", "/a/:request_id/deny");
        assert_ne!("/a/:request_id", "/a/:request_id/approve");
    }
}
