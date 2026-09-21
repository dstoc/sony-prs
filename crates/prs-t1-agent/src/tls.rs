//! Process-wide TLS provider and secure-entropy initialization.

use rustls::crypto::{GetRandomFailed, SecureRandom};
use std::sync::OnceLock;

const ENTROPY_PROBE_BYTES: usize = 32;

static INITIALIZATION: OnceLock<Result<(), InitializationError>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InitializationError {
    EntropyUnavailable,
    ProviderAlreadyInstalled,
}

impl InitializationError {
    pub(crate) const fn kind(self) -> &'static str {
        match self {
            Self::EntropyUnavailable => "entropy_unavailable",
            Self::ProviderAlreadyInstalled => "provider_already_installed",
        }
    }
}

impl std::fmt::Display for InitializationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EntropyUnavailable => formatter.write_str(
                "secure OS entropy is unavailable; verify readable, initialized /dev/random and /dev/urandom",
            ),
            Self::ProviderAlreadyInstalled => {
                formatter.write_str("the process already has a different TLS crypto provider")
            }
        }
    }
}

impl std::error::Error for InitializationError {}

/// Select ring for this legacy ARM target and prove its OS entropy source works.
///
/// Rustls installs its provider for the lifetime of the process. Keep the
/// result so probe and synchronization attempts can share that provider
/// without trying to install it again.
pub(crate) fn initialize() -> Result<(), InitializationError> {
    *INITIALIZATION.get_or_init(|| {
        let provider = rustls::crypto::ring::default_provider();
        verify_entropy(provider.secure_random)?;
        provider
            .install_default()
            .map_err(|_| InitializationError::ProviderAlreadyInstalled)
    })
}

fn verify_entropy(source: &dyn SecureRandom) -> Result<(), InitializationError> {
    let mut entropy = [0u8; ENTROPY_PROBE_BYTES];
    source
        .fill(&mut entropy)
        .map_err(|_: GetRandomFailed| InitializationError::EntropyUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct TestRandom {
        succeeds: bool,
        calls: AtomicUsize,
    }

    impl SecureRandom for TestRandom {
        fn fill(&self, buffer: &mut [u8]) -> Result<(), GetRandomFailed> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.succeeds {
                buffer.fill(0xa5);
                Ok(())
            } else {
                Err(GetRandomFailed)
            }
        }
    }

    #[test]
    fn entropy_preflight_accepts_secure_source() {
        let source = TestRandom {
            succeeds: true,
            calls: AtomicUsize::new(0),
        };

        assert_eq!(verify_entropy(&source), Ok(()));
        assert_eq!(source.calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn entropy_preflight_fails_closed() {
        let source = TestRandom {
            succeeds: false,
            calls: AtomicUsize::new(0),
        };

        assert_eq!(
            verify_entropy(&source),
            Err(InitializationError::EntropyUnavailable)
        );
        assert_eq!(source.calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn mixed_probe_and_sync_attempts_reuse_one_process_provider() {
        // The integrated sync path and the standalone network probe both call
        // initialize(). Exercise their mixed ordering and a repeated sync
        // attempt in the same process.
        for path in ["sync", "probe", "sync"] {
            assert_eq!(initialize(), Ok(()), "TLS initialization failed for {path}");
        }
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }
}
