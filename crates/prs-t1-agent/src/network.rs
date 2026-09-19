use hickory_resolver::config::{ResolverConfig, GOOGLE};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::TokioResolver;
use reqwest::redirect::Policy;
use reqwest::{Client, Response, Url};
use std::error::Error as StdError;
use std::fmt;
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::runtime::Builder as RuntimeBuilder;
use tokio::time::timeout;

const DNS_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;
const NEGATIVE_HOSTNAME: &str = "invalid.prs-t1.invalid";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeConfig {
    endpoint: Url,
    invalid_hostname: bool,
}

impl ProbeConfig {
    fn parse(args: &[String]) -> Result<Self, ProbeFailure> {
        let mut values = args.iter();
        let endpoint = values
            .next()
            .ok_or_else(|| ProbeFailure::usage("network-probe requires a hostname or HTTPS URL"))?;
        let invalid_hostname = match values.next().map(String::as_str) {
            None => false,
            Some("--invalid-hostname") => true,
            Some(value) => {
                return Err(ProbeFailure::usage(format!(
                    "unknown network-probe option {value:?}; expected --invalid-hostname"
                )))
            }
        };
        if values.next().is_some() {
            return Err(ProbeFailure::usage(
                "network-probe accepts HOSTNAME_OR_HTTPS_URL and optional --invalid-hostname",
            ));
        }

        let endpoint = if endpoint.contains("://") {
            endpoint.clone()
        } else {
            format!("https://{endpoint}")
        };
        let endpoint = Url::parse(&endpoint).map_err(|error| {
            ProbeFailure::configuration(format!("invalid HTTPS endpoint: {error}"))
        })?;
        if endpoint.scheme() != "https" {
            return Err(ProbeFailure::configuration(
                "network-probe requires an https:// endpoint",
            ));
        }
        if endpoint.host_str().is_none() {
            return Err(ProbeFailure::configuration(
                "network-probe endpoint must contain a hostname",
            ));
        }
        if !endpoint.username().is_empty() || endpoint.password().is_some() {
            return Err(ProbeFailure::configuration(
                "network-probe does not accept credentials in the endpoint",
            ));
        }
        if endpoint.query().is_some() || endpoint.fragment().is_some() {
            return Err(ProbeFailure::configuration(
                "network-probe endpoint must not contain a query or fragment",
            ));
        }

        Ok(Self {
            endpoint,
            invalid_hostname,
        })
    }

    fn health_url(&self) -> Result<Url, ProbeFailure> {
        let mut url = self.endpoint.clone();
        url.set_path("/health");
        if self.invalid_hostname {
            url.set_host(Some(NEGATIVE_HOSTNAME)).map_err(|error| {
                ProbeFailure::configuration(format!("could not construct negative URL: {error}"))
            })?;
        }
        Ok(url)
    }
}

#[derive(Debug)]
struct ProbeFailure {
    stage: &'static str,
    kind: &'static str,
    detail: String,
}

impl ProbeFailure {
    fn usage(detail: impl Into<String>) -> Self {
        Self {
            stage: "arguments",
            kind: "usage",
            detail: detail.into(),
        }
    }

    fn configuration(detail: impl Into<String>) -> Self {
        Self {
            stage: "configuration",
            kind: "invalid_input",
            detail: detail.into(),
        }
    }

    fn runtime(detail: impl Into<String>) -> Self {
        Self {
            stage: "runtime",
            kind: "runtime_failed",
            detail: detail.into(),
        }
    }

    fn dns(detail: impl Into<String>) -> Self {
        Self {
            stage: "dns",
            kind: "resolution_failed",
            detail: detail.into(),
        }
    }

    fn dns_timeout(detail: impl Into<String>) -> Self {
        Self {
            stage: "dns",
            kind: "resolution_timeout",
            detail: detail.into(),
        }
    }

    fn request(error: reqwest::Error) -> Self {
        let hostname_validation_failure = is_expected_hostname_validation_failure(&error);
        let kind = if error.is_timeout() {
            "timeout"
        } else if hostname_validation_failure {
            "tls_hostname_validation_failed"
        } else if error.is_connect() {
            "connect_failed"
        } else {
            "request_failed"
        };
        Self {
            stage: "https",
            kind,
            detail: error.to_string(),
        }
    }

    fn request_timeout(detail: impl Into<String>) -> Self {
        Self {
            stage: "https",
            kind: "timeout",
            detail: detail.into(),
        }
    }

    fn response(detail: impl Into<String>) -> Self {
        Self {
            stage: "response",
            kind: "response_failed",
            detail: detail.into(),
        }
    }

    fn response_timeout(detail: impl Into<String>) -> Self {
        Self {
            stage: "response",
            kind: "timeout",
            detail: detail.into(),
        }
    }
}

impl fmt::Display for ProbeFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for ProbeFailure {}

#[derive(Debug)]
struct HealthResponse {
    status: u16,
    bytes: Vec<u8>,
    addresses: Vec<SocketAddr>,
}

pub fn run(args: Vec<String>) -> io::Result<()> {
    let config = match ProbeConfig::parse(&args) {
        Ok(config) => config,
        Err(error) => {
            print_failure(&error);
            return Err(io::Error::new(io::ErrorKind::InvalidInput, error));
        }
    };

    let runtime = RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            let failure = ProbeFailure::runtime(format!("could not create async runtime: {error}"));
            print_failure(&failure);
            io::Error::other(failure)
        })?;

    match runtime.block_on(probe(&config)) {
        Ok(response) => {
            print_success(&config, &response);
            Ok(())
        }
        Err(error) if config.invalid_hostname && error.kind == "tls_hostname_validation_failed" => {
            print_expected_negative(&config, &error);
            Ok(())
        }
        Err(error) => {
            print_failure(&error);
            Err(io::Error::other(error))
        }
    }
}

async fn probe(config: &ProbeConfig) -> Result<HealthResponse, ProbeFailure> {
    let source_host = config
        .endpoint
        .host_str()
        .ok_or_else(|| ProbeFailure::configuration("endpoint has no hostname"))?;
    let port = config.endpoint.port_or_known_default().ok_or_else(|| {
        ProbeFailure::configuration("endpoint must use a known HTTPS port or specify one")
    })?;
    let addresses = resolve_host(source_host, port).await?;

    let url = config.health_url()?;
    let request_host = url
        .host_str()
        .ok_or_else(|| ProbeFailure::configuration("health URL has no hostname"))?;
    let client = Client::builder()
        .tls_backend_rustls()
        .tls_certs_only(trusted_certificates()?)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .read_timeout(RESPONSE_TIMEOUT)
        .redirect(Policy::none())
        .user_agent(concat!("prs-t1-agent/", env!("CARGO_PKG_VERSION")))
        // Pin the request to the addresses reported above. The negative test
        // changes only the name used for SNI and certificate validation.
        .resolve_to_addrs(request_host, &addresses)
        .build()
        .map_err(ProbeFailure::request)?;
    let response = timeout(REQUEST_TIMEOUT, client.get(url).send())
        .await
        .map_err(|_| ProbeFailure::request_timeout("HTTPS request exceeded 20 seconds"))?
        .map_err(ProbeFailure::request)?;
    let response = read_response(response).await?;
    Ok(HealthResponse {
        addresses,
        ..response
    })
}

async fn resolve_host(host: &str, port: u16) -> Result<Vec<SocketAddr>, ProbeFailure> {
    // Hickory performs DNS over Tokio I/O. The current-thread runtime keeps
    // the probe single-threaded, which is required by the T1 bootstrap target.
    let mut builder = match TokioResolver::builder_tokio() {
        Ok(builder) => builder,
        Err(_) => TokioResolver::builder_with_config(
            ResolverConfig::udp_and_tcp(&GOOGLE),
            TokioRuntimeProvider::default(),
        ),
    };
    builder.options_mut().timeout = DNS_TIMEOUT;
    builder.options_mut().attempts = 1;
    let resolver = builder
        .build()
        .map_err(|error| ProbeFailure::dns(format!("could not create resolver: {error}")))?;
    let host_for_error = host.to_owned();
    let query_host = host.to_owned();
    resolve_host_with_timeout(host_for_error, port, DNS_TIMEOUT, move || async move {
        let lookup = resolver
            .lookup_ip(query_host.as_str())
            .await
            .map_err(|error| error.to_string())?;
        Ok(lookup.iter().collect())
    })
    .await
}

async fn resolve_host_with_timeout<F, Fut>(
    host: String,
    port: u16,
    duration: Duration,
    resolver: F,
) -> Result<Vec<SocketAddr>, ProbeFailure>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<IpAddr>, String>>,
{
    match timeout(duration, resolver()).await {
        Ok(Ok(addresses)) if addresses.is_empty() => Err(ProbeFailure::dns(format!(
            "{host}:{port}: resolver returned no addresses"
        ))),
        Ok(Ok(addresses)) => Ok(addresses
            .into_iter()
            .map(|address| SocketAddr::new(address, port))
            .collect()),
        Ok(Err(error)) => Err(ProbeFailure::dns(format!("{host}:{port}: {error}"))),
        Err(_) => Err(ProbeFailure::dns_timeout(format!(
            "{host}:{port}: resolver did not return within {} seconds",
            duration.as_secs()
        ))),
    }
}

fn is_expected_hostname_validation_failure(error: &reqwest::Error) -> bool {
    error
        .source()
        .is_some_and(source_chain_has_hostname_mismatch)
}

fn source_chain_has_hostname_mismatch(error: &(dyn StdError + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(tls_error) = error.downcast_ref::<rustls::Error>() {
            return is_hostname_mismatch(tls_error);
        }
        if let Some(io_error) = error.downcast_ref::<io::Error>() {
            if let Some(inner) = io_error.get_ref() {
                if source_chain_has_hostname_mismatch(inner) {
                    return true;
                }
            }
        }
        current = error.source();
    }
    false
}

fn is_hostname_mismatch(error: &rustls::Error) -> bool {
    matches!(
        error,
        rustls::Error::InvalidCertificate(
            rustls::CertificateError::NotValidForName
                | rustls::CertificateError::NotValidForNameContext { .. }
        )
    )
}

fn trusted_certificates() -> Result<Vec<reqwest::Certificate>, ProbeFailure> {
    webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .map(|certificate| {
            reqwest::Certificate::from_der(certificate.as_ref()).map_err(|error| {
                ProbeFailure::configuration(format!("invalid bundled root certificate: {error}"))
            })
        })
        .collect()
}

async fn read_response(response: Response) -> Result<HealthResponse, ProbeFailure> {
    let status = response.status().as_u16();
    let content_length = response.content_length();
    if content_length.is_some_and(|length| length > MAX_RESPONSE_BYTES) {
        return Err(ProbeFailure::response(format!(
            "HTTP {status} response exceeds {MAX_RESPONSE_BYTES} bytes"
        )));
    }

    let body = timeout(RESPONSE_TIMEOUT, read_bounded_body(response, status))
        .await
        .map_err(|_| ProbeFailure::response_timeout("HTTP response exceeded 20 seconds"))??;
    if !(200..300).contains(&status) {
        return Err(ProbeFailure::response(format!(
            "health endpoint returned HTTP {status} with body {}",
            escape_bytes(&body)
        )));
    }
    Ok(HealthResponse {
        status,
        bytes: body,
        addresses: Vec::new(),
    })
}

async fn read_bounded_body(mut response: Response, status: u16) -> Result<Vec<u8>, ProbeFailure> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        ProbeFailure::response(format!("could not read HTTP {status} body: {error}"))
    })? {
        append_bounded_body(&mut body, &chunk, status)?;
    }
    Ok(body)
}

fn append_bounded_body(body: &mut Vec<u8>, chunk: &[u8], status: u16) -> Result<(), ProbeFailure> {
    let new_length = body.len().checked_add(chunk.len());
    if new_length.is_none_or(|length| length as u64 > MAX_RESPONSE_BYTES) {
        return Err(ProbeFailure::response(format!(
            "HTTP {status} response exceeds {MAX_RESPONSE_BYTES} bytes"
        )));
    }
    body.extend_from_slice(chunk);
    Ok(())
}

fn print_success(config: &ProbeConfig, response: &HealthResponse) {
    let host = config.endpoint.host_str().unwrap_or("<unknown>");
    println!("probe=network");
    println!("endpoint_host={host}");
    println!("endpoint_path=/health");
    println!("mode=normal");
    println!(
        "dns_addresses={}",
        response
            .addresses
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    );
    println!("tls_validation=passed");
    println!("http_status={}", response.status);
    println!("response_bytes={}", response.bytes.len());
    println!("response_body={}", escape_bytes(&response.bytes));
    println!("connect_timeout_seconds={}", CONNECT_TIMEOUT.as_secs());
    println!("request_timeout_seconds={}", REQUEST_TIMEOUT.as_secs());
    println!("response_timeout_seconds={}", RESPONSE_TIMEOUT.as_secs());
    println!("dns_timeout_seconds={}", DNS_TIMEOUT.as_secs());
    println!("response_limit_bytes={MAX_RESPONSE_BYTES}");
    println!("result=success");
}

fn print_expected_negative(config: &ProbeConfig, error: &ProbeFailure) {
    let source_host = config.endpoint.host_str().unwrap_or("<unknown>");
    println!("probe=network");
    println!("endpoint_host={source_host}");
    println!("endpoint_path=/health");
    println!("mode=hostname_negative");
    println!("tested_hostname={NEGATIVE_HOSTNAME}");
    println!("tls_validation=failed_as_expected");
    println!("failure_stage={}", error.stage);
    println!("failure_kind={}", error.kind);
    println!("error={}", escape_bytes(error.detail.as_bytes()));
    println!("result=success");
}

fn print_failure(error: &ProbeFailure) {
    println!("probe=network");
    println!("result=failure");
    println!("failure_stage={}", error.stage);
    println!("failure_kind={}", error.kind);
    println!("error={}", escape_bytes(error.detail.as_bytes()));
}

fn escape_bytes(bytes: &[u8]) -> String {
    let mut escaped = String::with_capacity(bytes.len() + 2);
    escaped.push('"');
    for byte in bytes {
        match byte {
            b'"' => escaped.push_str("\\\""),
            b'\\' => escaped.push_str("\\\\"),
            b'\n' => escaped.push_str("\\n"),
            b'\r' => escaped.push_str("\\r"),
            b'\t' => escaped.push_str("\\t"),
            0x20..=0x7e => escaped.push(*byte as char),
            _ => escaped.push_str(&format!("\\x{byte:02x}")),
        }
    }
    escaped.push('"');
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn test_runtime() -> tokio::runtime::Runtime {
        RuntimeBuilder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn accepts_a_hostname_and_forces_health_path() {
        let config = ProbeConfig::parse(&["reader.example.test/api/v1".into()]).unwrap();

        assert_eq!(
            config.endpoint.as_str(),
            "https://reader.example.test/api/v1"
        );
        assert_eq!(
            config.health_url().unwrap().as_str(),
            "https://reader.example.test/health"
        );
    }

    #[test]
    fn rejects_non_https_and_credentials() {
        let http = ProbeConfig::parse(&["http://reader.example.test".into()]).unwrap_err();
        assert_eq!(http.kind, "invalid_input");

        let credentials =
            ProbeConfig::parse(&["https://user:password@reader.example.test".into()]).unwrap_err();
        assert_eq!(credentials.kind, "invalid_input");
    }

    #[test]
    fn negative_mode_replaces_only_the_tls_hostname() {
        let config = ProbeConfig::parse(&[
            "https://reader.example.test:8443/service".into(),
            "--invalid-hostname".into(),
        ])
        .unwrap();

        let url = config.health_url().unwrap();
        assert_eq!(url.as_str(), "https://invalid.prs-t1.invalid:8443/health");
        assert!(config.invalid_hostname);
    }

    #[test]
    fn response_body_limit_accepts_exact_limit_and_rejects_over_limit() {
        let mut exact = Vec::new();
        append_bounded_body(&mut exact, &vec![b'x'; MAX_RESPONSE_BYTES as usize], 200).unwrap();
        assert_eq!(exact.len() as u64, MAX_RESPONSE_BYTES);

        let mut over = exact;
        let error = append_bounded_body(&mut over, b"x", 200).unwrap_err();
        assert_eq!(error.stage, "response");
    }

    #[test]
    fn output_escapes_control_bytes() {
        assert_eq!(escape_bytes(b"ok\n\"x"), r#""ok\n\"x""#);
    }

    #[test]
    fn output_escapes_non_utf8_response_bytes() {
        assert_eq!(escape_bytes(&[0xff, b'a']), r#""\xffa""#);
    }

    #[test]
    fn dns_resolution_has_a_bounded_wait() {
        let runtime = test_runtime();
        let error = runtime.block_on(resolve_host_with_timeout(
            "reader.example.test".into(),
            443,
            Duration::from_millis(5),
            || async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(Vec::<IpAddr>::new())
            },
        ));
        let error = error.unwrap_err();

        assert_eq!(error.stage, "dns");
        assert_eq!(error.kind, "resolution_timeout");
    }

    #[test]
    fn dns_resolution_reports_resolver_failures() {
        let runtime = test_runtime();
        let error = runtime.block_on(resolve_host_with_timeout(
            "reader.example.test".into(),
            443,
            Duration::from_secs(1),
            || async { Err("no DNS answer".into()) },
        ));
        let error = error.unwrap_err();

        assert_eq!(error.stage, "dns");
        assert_eq!(error.kind, "resolution_failed");
        assert!(error.detail.contains("no DNS answer"));
    }

    #[test]
    fn hostname_negative_accepts_only_a_hostname_mismatch() {
        let mismatch = io::Error::new(
            io::ErrorKind::InvalidData,
            rustls::Error::InvalidCertificate(rustls::CertificateError::NotValidForName),
        );
        let expired = io::Error::new(
            io::ErrorKind::InvalidData,
            rustls::Error::InvalidCertificate(rustls::CertificateError::Expired),
        );
        let unknown_issuer = io::Error::new(
            io::ErrorKind::InvalidData,
            rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer),
        );
        let refused = io::Error::new(io::ErrorKind::ConnectionRefused, "connection refused");

        assert!(source_chain_has_hostname_mismatch(&mismatch));
        assert!(!source_chain_has_hostname_mismatch(&expired));
        assert!(!source_chain_has_hostname_mismatch(&unknown_issuer));
        assert!(!source_chain_has_hostname_mismatch(&refused));
    }
}
