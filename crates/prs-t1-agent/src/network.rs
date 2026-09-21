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
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Builder as RuntimeBuilder;
use tokio::time::timeout;

const DNS_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;
const NEGATIVE_HOSTNAME: &str = "invalid.prs-t1.invalid";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NetworkFault {
    Dns,
    Tls,
    Response,
}

impl NetworkFault {
    fn parse(value: &str) -> Result<Self, ProbeFailure> {
        match value {
            "dns" => Ok(Self::Dns),
            "tls" => Ok(Self::Tls),
            "response" => Ok(Self::Response),
            _ => Err(ProbeFailure::usage(format!(
                "unknown network-loss stage {value:?}; expected dns, tls, or response"
            ))),
        }
    }

    const fn stage(self) -> &'static str {
        match self {
            Self::Dns => "dns",
            Self::Tls => "tls",
            Self::Response => "response",
        }
    }

    const fn kind(self) -> &'static str {
        match self {
            Self::Dns => "injected_dns_network_loss",
            Self::Tls => "injected_tls_network_loss",
            Self::Response => "injected_response_network_loss",
        }
    }

    fn failure(self, detail: impl Into<String>) -> ProbeFailure {
        ProbeFailure {
            stage: self.stage(),
            kind: self.kind(),
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeConfig {
    endpoint: Url,
    invalid_hostname: bool,
    fault: Option<NetworkFault>,
}

impl ProbeConfig {
    fn parse(args: &[String]) -> Result<Self, ProbeFailure> {
        let mut values = args.iter();
        let endpoint = values
            .next()
            .ok_or_else(|| ProbeFailure::usage("network-probe requires a hostname or HTTPS URL"))?;
        let mut invalid_hostname = false;
        let mut fault = None;
        while let Some(value) = values.next() {
            match value.as_str() {
                "--invalid-hostname" => invalid_hostname = true,
                "--inject-network-loss" => {
                    let stage = values.next().ok_or_else(|| {
                        ProbeFailure::usage(
                            "--inject-network-loss requires dns, tls, or response",
                        )
                    })?;
                    if fault.replace(NetworkFault::parse(stage)?).is_some() {
                        return Err(ProbeFailure::usage(
                            "network-probe accepts only one --inject-network-loss stage",
                        ));
                    }
                }
                value => {
                    return Err(ProbeFailure::usage(format!(
                        "unknown network-probe option {value:?}; expected --invalid-hostname or --inject-network-loss STAGE"
                    )))
                }
            }
        }
        if invalid_hostname && fault.is_some() {
            return Err(ProbeFailure::usage(
                "--invalid-hostname cannot be combined with --inject-network-loss",
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
            fault,
        })
    }

    fn health_url(&self) -> Result<Url, ProbeFailure> {
        let mut url = self.endpoint.clone();
        url.set_path("/health");
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

    fn tls_initialization(error: crate::tls::InitializationError) -> Self {
        Self {
            stage: "tls",
            kind: error.kind(),
            detail: error.to_string(),
        }
    }

    fn tls_configuration(detail: impl Into<String>) -> Self {
        Self {
            stage: "tls",
            kind: "configuration_failed",
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
        let (stage, kind) = if error.is_timeout() {
            ("https", "timeout")
        } else if hostname_validation_failure {
            ("tls", "tls_hostname_validation_failed")
        } else if error.is_connect() {
            ("https", "connect_failed")
        } else {
            ("https", "request_failed")
        };
        Self {
            stage,
            kind,
            detail: error.to_string(),
        }
    }

    fn request_for(error: reqwest::Error, fault: Option<NetworkFault>) -> Self {
        if fault == Some(NetworkFault::Tls) && source_chain_has_tls_error(&error) {
            return NetworkFault::Tls
                .failure(format!("injected TLS negotiation failure: {}", error));
        }
        Self::request(error)
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

    if let Some(fault) = config.fault {
        println!(
            "fault_injection=network_loss failure_stage={} failure_kind={}",
            fault.stage(),
            fault.kind()
        );
    }

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
        Err(error) if is_expected_negative_failure(&config, &error) => {
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
    if config.fault == Some(NetworkFault::Dns) {
        return Err(NetworkFault::Dns
            .failure("DNS resolution interrupted by the diagnostic network-loss injection"));
    }
    crate::tls::initialize().map_err(ProbeFailure::tls_initialization)?;

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
    let client = if config.invalid_hostname {
        Client::builder()
            .tls_backend_rustls()
            .tls_backend_preconfigured(invalid_hostname_tls_config()?)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .read_timeout(RESPONSE_TIMEOUT)
            .redirect(Policy::none())
            .user_agent(concat!("prs-t1-agent/", env!("CARGO_PKG_VERSION")))
            .resolve_to_addrs(request_host, &addresses)
            .build()
            .map_err(ProbeFailure::request)?
    } else if config.fault == Some(NetworkFault::Tls) {
        Client::builder()
            .tls_backend_rustls()
            .tls_backend_preconfigured(injected_tls_config()?)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .read_timeout(RESPONSE_TIMEOUT)
            .redirect(Policy::none())
            .user_agent(concat!("prs-t1-agent/", env!("CARGO_PKG_VERSION")))
            .resolve_to_addrs(request_host, &addresses)
            .build()
            .map_err(ProbeFailure::request)?
    } else {
        build_https_client(request_host, &addresses).map_err(ProbeFailure::runtime)?
    };
    let response = timeout(REQUEST_TIMEOUT, client.get(url).send())
        .await
        .map_err(|_| ProbeFailure::request_timeout("HTTPS request exceeded 20 seconds"))?
        .map_err(|error| ProbeFailure::request_for(error, config.fault))?;
    let response = read_response(response, config.fault).await?;
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

/// Resolve a host once so the HTTPS client can pin its connections to the
/// result. The T1 uses one current-thread runtime for both DNS and requests.
pub(crate) async fn resolve_host_addresses(
    host: &str,
    port: u16,
) -> Result<Vec<SocketAddr>, String> {
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
        .map_err(|error| format!("could not create resolver: {error}"))?;
    let host_for_error = host.to_owned();
    let query_host = host.to_owned();
    match timeout(DNS_TIMEOUT, async move {
        let lookup = resolver
            .lookup_ip(query_host.as_str())
            .await
            .map_err(|error| error.to_string())?;
        Ok::<Vec<IpAddr>, String>(lookup.iter().collect::<Vec<_>>())
    })
    .await
    {
        Ok(Ok(addresses)) if addresses.is_empty() => Err(format!(
            "{host_for_error}:{port}: resolver returned no addresses"
        )),
        Ok(Ok(addresses)) => Ok(addresses
            .into_iter()
            .map(|address| SocketAddr::new(address, port))
            .collect()),
        Ok(Err(error)) => Err(format!("{host_for_error}:{port}: {error}")),
        Err(_) => Err(format!(
            "{host_for_error}:{port}: resolver did not return within {} seconds",
            DNS_TIMEOUT.as_secs()
        )),
    }
}

/// Build the normal pinned HTTPS client used by both the network probe and
/// the PRSync client. Redirects are disabled because an approval or reader
/// token must never be forwarded to another origin.
pub(crate) fn build_https_client(host: &str, addresses: &[SocketAddr]) -> Result<Client, String> {
    Client::builder()
        .tls_backend_rustls()
        .tls_certs_only(trusted_certificates().map_err(|error| error.to_string())?)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .read_timeout(RESPONSE_TIMEOUT)
        .redirect(Policy::none())
        .user_agent(concat!("prs-t1-agent/", env!("CARGO_PKG_VERSION")))
        .resolve_to_addrs(host, addresses)
        .build()
        .map_err(|error| error.to_string())
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

fn source_chain_has_tls_error(error: &(dyn StdError + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if error.downcast_ref::<rustls::Error>().is_some() {
            return true;
        }
        if let Some(io_error) = error.downcast_ref::<io::Error>() {
            if let Some(inner) = io_error.get_ref() {
                if source_chain_has_tls_error(inner) {
                    return true;
                }
            }
        }
        current = error.source();
    }
    false
}

fn source_chain_has_hostname_mismatch(error: &(dyn StdError + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(tls_error) = error.downcast_ref::<rustls::Error>() {
            if is_hostname_mismatch(tls_error) {
                return true;
            }
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

#[derive(Debug)]
struct InvalidHostnameVerifier {
    inner: Arc<rustls::client::WebPkiServerVerifier>,
    expected_name: rustls::pki_types::ServerName<'static>,
}

impl rustls::client::danger::ServerCertVerifier for InvalidHostnameVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        ocsp_response: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        self.inner.verify_server_cert(
            end_entity,
            intermediates,
            &self.expected_name,
            ocsp_response,
            now,
        )
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

fn invalid_hostname_tls_config() -> Result<rustls::ClientConfig, ProbeFailure> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    for certificate in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        roots.add(certificate.clone()).map_err(|error| {
            ProbeFailure::tls_configuration(format!("invalid bundled root certificate: {error}"))
        })?;
    }
    let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        provider.clone(),
    )
    .build()
    .map_err(|error| {
        ProbeFailure::tls_configuration(format!("could not build certificate verifier: {error}"))
    })?;
    let expected_name = rustls::pki_types::ServerName::try_from(NEGATIVE_HOSTNAME.to_owned())
        .map_err(|error| {
            ProbeFailure::tls_configuration(format!(
                "invalid negative-test hostname {NEGATIVE_HOSTNAME:?}: {error}"
            ))
        })?;

    // The dangerous builder is required to substitute the expected name. The
    // wrapper delegates chain, trust-root, validity, and signature checks to
    // WebPki; it never accepts an invalid certificate.
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| {
            ProbeFailure::tls_configuration(format!(
                "could not configure TLS protocol versions: {error}"
            ))
        })?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(InvalidHostnameVerifier {
            inner: verifier,
            expected_name,
        }))
        .with_no_client_auth())
}

#[derive(Debug)]
struct InjectedTlsFailureVerifier {
    inner: Arc<rustls::client::WebPkiServerVerifier>,
}

impl rustls::client::danger::ServerCertVerifier for InjectedTlsFailureVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Err(rustls::Error::General(
            "injected TLS negotiation network loss".into(),
        ))
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

fn injected_tls_config() -> Result<rustls::ClientConfig, ProbeFailure> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    for certificate in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        roots.add(certificate.clone()).map_err(|error| {
            ProbeFailure::tls_configuration(format!("invalid bundled root certificate: {error}"))
        })?;
    }
    let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        provider.clone(),
    )
    .build()
    .map_err(|error| {
        ProbeFailure::tls_configuration(format!("could not build certificate verifier: {error}"))
    })?;

    // The wrapper deliberately fails during the real server-certificate step.
    // It does not disable verification or accept a certificate.
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| {
            ProbeFailure::tls_configuration(format!(
                "could not configure TLS protocol versions: {error}"
            ))
        })?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(InjectedTlsFailureVerifier { inner: verifier }))
        .with_no_client_auth())
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

async fn read_response(
    response: Response,
    fault: Option<NetworkFault>,
) -> Result<HealthResponse, ProbeFailure> {
    let status = response.status().as_u16();
    let content_length = response.content_length();
    if content_length.is_some_and(|length| length > MAX_RESPONSE_BYTES) {
        return Err(ProbeFailure::response(format!(
            "HTTP {status} response exceeds {MAX_RESPONSE_BYTES} bytes"
        )));
    }

    let body = timeout(RESPONSE_TIMEOUT, read_bounded_body(response, status, fault))
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

async fn read_bounded_body(
    mut response: Response,
    status: u16,
    fault: Option<NetworkFault>,
) -> Result<Vec<u8>, ProbeFailure> {
    if fault == Some(NetworkFault::Response) {
        return Err(NetworkFault::Response
            .failure("response body read interrupted by the diagnostic network-loss injection"));
    }
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
    println!("sni_hostname={source_host}");
    println!("tested_hostname={NEGATIVE_HOSTNAME}");
    println!("tls_validation=failed_as_expected");
    println!("failure_stage={}", error.stage);
    println!("failure_kind={}", error.kind);
    println!("error={}", escape_bytes(error.detail.as_bytes()));
    println!("result=success");
}

fn is_expected_negative_failure(config: &ProbeConfig, error: &ProbeFailure) -> bool {
    config.invalid_hostname && error.kind == "tls_hostname_validation_failed"
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
    fn parses_one_explicit_network_loss_stage() {
        for (stage, expected) in [
            ("dns", NetworkFault::Dns),
            ("tls", NetworkFault::Tls),
            ("response", NetworkFault::Response),
        ] {
            let config = ProbeConfig::parse(&[
                "reader.example.test".into(),
                "--inject-network-loss".into(),
                stage.into(),
            ])
            .unwrap();

            assert_eq!(config.fault, Some(expected));
            assert!(!config.invalid_hostname);
        }
    }

    #[test]
    fn rejects_ambiguous_or_unknown_network_loss_options() {
        let unknown = ProbeConfig::parse(&[
            "reader.example.test".into(),
            "--inject-network-loss".into(),
            "connect".into(),
        ])
        .unwrap_err();
        assert_eq!(unknown.kind, "usage");

        let combined = ProbeConfig::parse(&[
            "reader.example.test".into(),
            "--invalid-hostname".into(),
            "--inject-network-loss".into(),
            "tls".into(),
        ])
        .unwrap_err();
        assert_eq!(combined.kind, "usage");
    }

    #[test]
    fn network_loss_stages_have_distinct_structured_results() {
        let results = [
            (NetworkFault::Dns, "dns", "injected_dns_network_loss"),
            (NetworkFault::Tls, "tls", "injected_tls_network_loss"),
            (
                NetworkFault::Response,
                "response",
                "injected_response_network_loss",
            ),
        ];

        for (fault, stage, kind) in results {
            let error = fault.failure("test injection");
            assert_eq!((error.stage, error.kind), (stage, kind));
        }
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
    fn negative_mode_keeps_the_production_endpoint_for_sni() {
        let config = ProbeConfig::parse(&[
            "https://reader.example.test:8443/service".into(),
            "--invalid-hostname".into(),
        ])
        .unwrap();

        let url = config.health_url().unwrap();
        assert_eq!(url.as_str(), "https://reader.example.test:8443/health");
        assert_eq!(NEGATIVE_HOSTNAME, "invalid.prs-t1.invalid");
        assert!(config.invalid_hostname);
    }

    #[test]
    fn negative_tls_config_builds_from_bundled_roots() {
        let _config = invalid_hostname_tls_config().unwrap();
    }

    #[test]
    fn injected_tls_config_keeps_a_real_verifier_boundary() {
        let _config = injected_tls_config().unwrap();
    }

    #[test]
    fn negative_mode_accepts_only_hostname_validation_failures() {
        let config = ProbeConfig::parse(&[
            "https://reader.example.test".into(),
            "--invalid-hostname".into(),
        ])
        .unwrap();
        let expected = ProbeFailure {
            stage: "tls",
            kind: "tls_hostname_validation_failed",
            detail: "certificate name mismatch".into(),
        };
        let connect = ProbeFailure {
            stage: "https",
            kind: "connect_failed",
            detail: "connection refused".into(),
        };
        let expired = ProbeFailure {
            stage: "tls",
            kind: "request_failed",
            detail: "certificate expired".into(),
        };

        assert!(is_expected_negative_failure(&config, &expected));
        assert!(!is_expected_negative_failure(&config, &connect));
        assert!(!is_expected_negative_failure(&config, &expired));
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
    fn entropy_failure_is_reported_as_a_structured_tls_failure() {
        let error =
            ProbeFailure::tls_initialization(crate::tls::InitializationError::EntropyUnavailable);

        assert_eq!(error.stage, "tls");
        assert_eq!(error.kind, "entropy_unavailable");
        assert!(error.detail.contains("secure OS entropy"));
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
