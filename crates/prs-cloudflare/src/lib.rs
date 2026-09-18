//! The PRSync Cloudflare Worker entry point.
//!
//! The Worker owns all D1 and R2 access. The protocol crate remains a
//! Cloudflare-independent collection of wire types.

use prs_sync_protocol::CURRENT_PROTOCOL_VERSION;
use worker::*;

mod storage;

pub const D1_BINDING: &str = "DB";
pub const R2_BINDING: &str = "BUNDLES";

fn require_bindings(env: &Env) -> Result<()> {
    let _store = bundle_store(env)?;
    Ok(())
}

pub(crate) fn bundle_store(env: &Env) -> Result<storage::BundleStore> {
    Ok(storage::BundleStore::from_env(
        env.d1(D1_BINDING)?,
        env.bucket(R2_BINDING)?,
    ))
}

#[event(fetch)]
pub async fn fetch(request: Request, env: Env, _context: Context) -> Result<Response> {
    require_bindings(&env)?;

    Router::new()
        .get_async("/", |_, _| async { Response::ok("PRSync Worker") })
        .get_async("/health", |_, _| async {
            Response::ok(format!(
                "prs-cloudflare {}.{}",
                CURRENT_PROTOCOL_VERSION.major, CURRENT_PROTOCOL_VERSION.minor
            ))
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
}
