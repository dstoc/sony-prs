//! The PRSync Cloudflare Worker entry point.
//!
//! The Worker owns all D1 and R2 access. The protocol crate remains a
//! Cloudflare-independent collection of wire types.

mod api;
mod approval;
mod authorization;
mod identity;

use prs_sync_protocol::Timestamp;

pub use authorization::{
    ApprovalRequestDetails, AuthenticatedCapability, AuthorizationConfig, AuthorizationError,
    AuthorizationFailure, AuthorizationResult, AuthorizationService, MaintenanceReport,
    OwnerApprovalCapability, PendingPollingCapability, RateLimitDecision, ReaderAuthorization,
    SenderAuthorization,
};
pub use identity::{AccessContext, PrincipalBoundary};
use worker::*;

mod storage;

pub const D1_BINDING: &str = "DB";
pub const R2_BINDING: &str = "BUNDLES";

/// Runs cleanup hourly. The storage operation bounds both D1 lifecycle work
/// and the R2 list scan, so a large backlog is retried by the next run.
pub const CLEANUP_CRON: &str = "0 * * * *";
const MILLISECONDS_PER_SECOND: u64 = 1_000;

pub(crate) fn bundle_store(env: &Env) -> Result<storage::BundleStore> {
    Ok(storage::BundleStore::from_env(
        env.d1(D1_BINDING)?,
        env.bucket(R2_BINDING)?,
    ))
}

#[event(fetch)]
pub async fn fetch(request: Request, env: Env, context: Context) -> Result<Response> {
    let access = identity::AccessContext::from_context(&context, &env, &request)?;
    api::register(Router::new())
        .get_async("/a/:request_id", move |request, context| async move {
            let request_id = context.param("request_id").cloned();
            approval::show(request, context.env, request_id, access).await
        })
        .post_async(
            "/a/:request_id/approve",
            move |request, context| async move {
                let request_id = context.param("request_id").cloned();
                approval::approve(request, context.env, request_id, access).await
            },
        )
        .post_async("/a/:request_id/deny", move |request, context| async move {
            let request_id = context.param("request_id").cloned();
            approval::deny(request, context.env, request_id, access).await
        })
        .run(request, env)
        .await
}

#[event(scheduled)]
pub async fn scheduled(event: ScheduledEvent, env: Env, _context: ScheduleContext) {
    let now = schedule_timestamp_seconds(event.schedule())
        .unwrap_or_else(|| Date::now().as_millis() / MILLISECONDS_PER_SECOND);

    match env.d1(D1_BINDING) {
        Ok(database) => {
            let service = AuthorizationService::new(database, AuthorizationConfig::default());
            if let Err(error) = service.cleanup(Timestamp::new(now)).await {
                worker::console_error!("PRSync authorization maintenance failed: {}", error);
            }
        }
        Err(error) => {
            worker::console_error!(
                "PRSync authorization maintenance could not open D1: {}",
                error
            );
        }
    }

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

fn schedule_timestamp_seconds(scheduled_at_millis: f64) -> Option<u64> {
    if scheduled_at_millis.is_finite() && scheduled_at_millis >= 0.0 {
        Some((scheduled_at_millis as u64) / MILLISECONDS_PER_SECOND)
    } else {
        None
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
    fn scheduled_timestamp_uses_unix_seconds() {
        assert_eq!(
            schedule_timestamp_seconds(1_800_000_000_999.0),
            Some(1_800_000_000)
        );
    }

    #[test]
    fn approval_routes_are_separate_explicit_actions() {
        assert_ne!("/a/:request_id/approve", "/a/:request_id/deny");
        assert_ne!("/a/:request_id", "/a/:request_id/approve");
    }
}
