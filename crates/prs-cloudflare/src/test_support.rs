//! Local-only controls for deterministic Worker integration tests.

use prs_sync_protocol::Timestamp;
use worker::{Env, Error, Request, Result};

pub(crate) const FAULT_HEADER: &str = "X-PRSync-Test-Fault";
pub(crate) const NOW_HEADER: &str = "X-PRSync-Test-Now";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FaultInjection {
    D1Clear,
    D1Publish,
    R2Put,
    R2Delete,
    R2Get,
    CleanupD1Claim,
    CleanupR2Delete,
}

impl FaultInjection {
    fn parse(value: &str) -> Result<Self> {
        let fault = match value {
            "d1-clear" => Self::D1Clear,
            "d1-publish" => Self::D1Publish,
            "r2-put" => Self::R2Put,
            "r2-delete" => Self::R2Delete,
            "r2-get" => Self::R2Get,
            "cleanup-d1-claim" => Self::CleanupD1Claim,
            "cleanup-r2-delete" => Self::CleanupR2Delete,
            _ => {
                return Err(Error::RustError(format!(
                    "unknown local test fault injection: {value}"
                )))
            }
        };
        Ok(fault)
    }
}

pub(crate) fn local_enabled(env: &Env) -> bool {
    env.var("PRS_ENVIRONMENT")
        .map(|value| value.to_string() == "local")
        .unwrap_or(false)
}

pub(crate) fn fault_for_request(request: &Request, env: &Env) -> Result<Option<FaultInjection>> {
    if !local_enabled(env) {
        return Ok(None);
    }
    request
        .headers()
        .get(FAULT_HEADER)?
        .map(|value| FaultInjection::parse(&value))
        .transpose()
}

pub(crate) fn timestamp_for_request(request: &Request, env: &Env) -> Timestamp {
    if local_enabled(env) {
        if let Ok(Some(value)) = request.headers().get(NOW_HEADER) {
            if let Ok(seconds) = value.parse::<u64>() {
                return Timestamp::new(seconds);
            }
        }
    }
    Timestamp::new(worker::Date::now().as_millis() as u64 / 1_000)
}

pub(crate) fn injected_failure(
    fault: Option<FaultInjection>,
    expected: FaultInjection,
) -> Result<()> {
    if fault == Some(expected) {
        return Err(Error::RustError(format!(
            "local test injected {:?} failure",
            expected
        )));
    }
    Ok(())
}
