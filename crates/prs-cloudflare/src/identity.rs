//! Cloudflare Access context handling for the human approval surface.
//!
//! Cloudflare authenticates direct Worker invocations before the Worker runs.
//! The runtime exposes that result as `ctx.access`. This module checks only
//! whether that trusted context exists. It does not read an Access assertion
//! header or implement JWT validation.

use worker::js_sys::Reflect;
use worker::wasm_bindgen::JsValue;
use worker::{Context, Env, Request};

#[cfg(feature = "local-test")]
pub const LOCAL_TEST_OWNER_HEADER: &str = "X-PRSync-Test-Owner";

#[cfg(feature = "local-test")]
const LOCAL_ENVIRONMENT: &str = "local";

/// A capability proving that Cloudflare Access authenticated this invocation.
///
/// The production value can only come from the runtime's `ctx.access` object.
/// The local-test feature also supports the explicit local identity header so
/// dependency-free tests can exercise the approval boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessContext {
    binding: String,
}

impl AccessContext {
    /// Return an Access capability when the runtime authenticated this request.
    ///
    /// Production builds never inspect the local test header. The local seam
    /// is selected only when both the feature and environment are explicit.
    pub fn from_context(
        context: &Context,
        _env: &Env,
        _request: &Request,
    ) -> worker::Result<Option<Self>> {
        #[cfg(feature = "local-test")]
        if local_test_enabled(_env) {
            if let Some(value) = _request.headers().get(LOCAL_TEST_OWNER_HEADER)? {
                validate_local_test_identity(&value)?;
                return Ok(Some(Self { binding: value }));
            }
        }

        let access = Reflect::get(context.as_ref().as_ref(), &JsValue::from_str("access"))?;
        if access.is_undefined() || access.is_null() {
            return Ok(None);
        } else {
            let audience = Reflect::get(&access, &JsValue::from_str("aud"))?
                .as_string()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    worker::Error::RustError(
                        "authenticated Access context has no audience".to_owned(),
                    )
                })?;
            Ok(Some(Self { binding: audience }))
        }
    }

    pub(crate) fn csrf_binding(&self) -> &str {
        &self.binding
    }

    #[cfg(test)]
    pub(crate) fn for_test(binding: impl Into<String>) -> Self {
        Self {
            binding: binding.into(),
        }
    }
}

/// Compatibility name for callers that refer to the approval trust boundary.
pub type PrincipalBoundary = AccessContext;

#[cfg(feature = "local-test")]
fn local_test_enabled(env: &Env) -> bool {
    env.var("PRS_ENVIRONMENT")
        .map(|value| value.to_string() == LOCAL_ENVIRONMENT)
        .unwrap_or(false)
}

#[cfg(feature = "local-test")]
fn validate_local_test_identity(value: &str) -> worker::Result<()> {
    let mut parts = value.split('|');
    let issuer = parts.next().unwrap_or_default();
    let subject = parts.next().unwrap_or_default();
    let email = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || issuer.is_empty()
        || subject.is_empty()
        || email.is_empty()
        || [issuer, subject, email]
            .into_iter()
            .any(|part| part.len() > 512 || part.chars().any(char::is_control))
    {
        return Err(worker::Error::RustError(
            "invalid local test Access identity".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "local-test")]
    use super::validate_local_test_identity;

    #[cfg(feature = "local-test")]
    #[test]
    fn local_identity_requires_three_bounded_fields() {
        assert!(
            validate_local_test_identity("https://access.example|owner-1|owner@example.com")
                .is_ok()
        );
        assert!(validate_local_test_identity("owner-1|owner@example.com").is_err());
        assert!(validate_local_test_identity("owner-1|owner@example.com|extra|field").is_err());
    }
}
