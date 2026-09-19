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

/// Runs cleanup hourly. The storage operation bounds both D1 lifecycle work
/// and the R2 list scan, so a large backlog is retried by the next run.
pub const CLEANUP_CRON: &str = "0 * * * *";

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

#[event(scheduled)]
pub async fn scheduled(event: ScheduledEvent, env: Env, _context: ScheduleContext) {
    let scheduled_at = event.schedule();
    let now = if scheduled_at.is_finite() && scheduled_at >= 0.0 {
        scheduled_at as u64
    } else {
        Date::now().as_millis()
    };

    match bundle_store(&env) {
        Ok(store) => match store.cleanup(now).await {
            Ok(report) => worker::console_log!("PRSync bundle cleanup completed: {:?}", report),
            Err(error) => worker::console_error!("PRSync bundle cleanup failed: {}", error),
        },
        Err(error) => {
            worker::console_error!("PRSync bundle cleanup could not open storage: {}", error)
        }
    }
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
    fn cleanup_schedule_is_hourly() {
        assert_eq!(CLEANUP_CRON, "0 * * * *");
    }

    #[test]
    fn approval_routes_are_separate_explicit_actions() {
        assert_ne!("/a/:request_id/approve", "/a/:request_id/deny");
        assert_ne!("/a/:request_id", "/a/:request_id/approve");
    }
}
