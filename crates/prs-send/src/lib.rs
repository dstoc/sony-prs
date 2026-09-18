//! The trusted PRSync sender command line client.
//!
//! The sender can publish and clear the hosted inbox and manage sender
//! credential metadata. It never calls a reader route and never persists a
//! bearer token.

use std::env;
use std::fmt;
use std::io::{self, Write};
use std::thread;
use std::time::Duration;

use prs_sync_bundle::{BundleBuilder, BundleError};
use prs_sync_protocol::{
    ApiError, AuthorizationStart, BearerToken, InboxRevision, PollingSecretClaimOutcome,
    PollingSecretClaimResult, ProtocolVersion, SenderCredentialMetadata, SenderCredentialName,
    Timestamp, CURRENT_PROTOCOL_VERSION,
};
use reqwest::blocking::{Client as HttpClient, Response};
use reqwest::{Method, StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

const BASE_URL_ENV: &str = "PRSYNC_URL";
const TOKEN_ENV: &str = "PRSYNC_SENDER_TOKEN";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub base_url: String,
    pub sender_token: Option<String>,
}

impl Config {
    pub fn from_environment() -> Result<Self, CliError> {
        let base_url = env::var(BASE_URL_ENV).map_err(|_| {
            CliError::Configuration(format!(
                "{BASE_URL_ENV} must contain the PRSync service URL"
            ))
        })?;
        let sender_token = env::var(TOKEN_ENV).ok();
        Ok(Self {
            base_url,
            sender_token,
        })
    }
}

#[derive(Debug)]
pub enum CliError {
    Usage(String),
    Configuration(String),
    InvalidInput(String),
    Io(io::Error),
    Bundle(BundleError),
    Json(serde_json::Error),
    Http(reqwest::Error),
    Server {
        status: StatusCode,
        error: Option<ApiError>,
    },
    Authorization(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message)
            | Self::Configuration(message)
            | Self::InvalidInput(message)
            | Self::Authorization(message) => formatter.write_str(message),
            Self::Io(error) => error.fmt(formatter),
            Self::Bundle(error) => write!(formatter, "could not create bundle: {error}"),
            Self::Json(error) => write!(formatter, "invalid service response: {error}"),
            Self::Http(error) => write!(formatter, "service request failed: {error}"),
            Self::Server { status, error } => match error {
                Some(error) => write!(
                    formatter,
                    "service returned HTTP {status} ({:?}): {}",
                    error.error.code, error.error.message
                ),
                None => write!(formatter, "service returned HTTP {status}"),
            },
        }
    }
}

impl std::error::Error for CliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Bundle(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Http(error) => Some(error),
            Self::Usage(_)
            | Self::Configuration(_)
            | Self::InvalidInput(_)
            | Self::Server { .. }
            | Self::Authorization(_) => None,
        }
    }
}

impl From<io::Error> for CliError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<BundleError> for CliError {
    fn from(error: BundleError) -> Self {
        Self::Bundle(error)
    }
}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    CredentialsCreate {
        name: String,
    },
    CredentialsList,
    CredentialsRevoke {
        name: String,
    },
    Push {
        entry_point: String,
        files: Vec<String>,
    },
    Clear,
}

impl Command {
    fn parse<I, S>(args: I) -> Result<Self, CliError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let args: Vec<String> = args.into_iter().map(Into::into).collect();
        match args.as_slice() {
            [command, subcommand, flag, name]
                if command == "credentials" && subcommand == "create" && flag == "--name" =>
            {
                if name.is_empty() {
                    return Err(CliError::Usage("credential name must not be empty".into()));
                }
                Ok(Self::CredentialsCreate { name: name.clone() })
            }
            [command, subcommand] if command == "credentials" && subcommand == "list" => {
                Ok(Self::CredentialsList)
            }
            [command, subcommand, name] if command == "credentials" && subcommand == "revoke" => {
                if name.is_empty() {
                    return Err(CliError::Usage("credential name must not be empty".into()));
                }
                Ok(Self::CredentialsRevoke { name: name.clone() })
            }
            [command, entry_point, files @ ..] if command == "push" => {
                if entry_point.is_empty() {
                    return Err(CliError::Usage("push requires an entry point".into()));
                }
                Ok(Self::Push {
                    entry_point: entry_point.clone(),
                    files: files.to_vec(),
                })
            }
            [command] if command == "clear" => Ok(Self::Clear),
            [command] if command == "help" || command == "--help" || command == "-h" => {
                Err(CliError::Usage(usage()))
            }
            _ => Err(CliError::Usage(usage())),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct SenderAuthorizationBody {
    protocol_version: ProtocolVersion,
    credential_name: SenderCredentialName,
}

#[derive(Debug, Clone, Serialize)]
struct CredentialRevokeBody {
    protocol_version: ProtocolVersion,
    name: SenderCredentialName,
}

#[derive(Debug, Clone, Serialize)]
struct PollRequest<'a> {
    protocol_version: ProtocolVersion,
    request_id: &'a prs_sync_protocol::AuthorizationRequestId,
    polling_secret: &'a prs_sync_protocol::PollingSecret,
}

#[derive(Debug, Clone, Deserialize)]
struct BundlePushResponse {
    revision: InboxRevision,
    etag: prs_sync_protocol::EntityTag,
    size_bytes: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct EmptyInboxResponse {
    revision: InboxRevision,
    state: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CredentialRevokeResponse {
    name: SenderCredentialName,
    revoked: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct CredentialListResponse {
    credentials: Vec<SenderCredentialMetadata>,
}

struct ServiceClient {
    http: HttpClient,
    base_url: Url,
    sender_token: Option<String>,
}

impl ServiceClient {
    fn new(config: Config) -> Result<Self, CliError> {
        let base_url = Url::parse(&format!("{}/", config.base_url.trim_end_matches('/')))
            .map_err(|error| CliError::Configuration(format!("invalid {BASE_URL_ENV}: {error}")))?;
        if !matches!(base_url.scheme(), "http" | "https") {
            return Err(CliError::Configuration(format!(
                "{BASE_URL_ENV} must use http or https"
            )));
        }
        let http = HttpClient::builder()
            .user_agent("prs-send/0.1")
            .build()
            .map_err(CliError::Http)?;
        Ok(Self {
            http,
            base_url,
            sender_token: config.sender_token,
        })
    }

    fn endpoint(&self, path: &str) -> Result<Url, CliError> {
        self.base_url
            .join(path.trim_start_matches('/'))
            .map_err(|error| CliError::Configuration(format!("invalid service path: {error}")))
    }

    fn sender_request(
        &self,
        method: Method,
        path: &str,
    ) -> Result<reqwest::blocking::RequestBuilder, CliError> {
        let token = self.sender_token.as_ref().ok_or_else(missing_token)?;
        let token = BearerToken::new(token.clone())
            .map_err(|error| CliError::Configuration(error.to_string()))?;
        Ok(self
            .http
            .request(method, self.endpoint(path)?)
            .bearer_auth(token.as_str()))
    }

    fn create_authorization(&self, name: &str) -> Result<AuthorizationStart, CliError> {
        let credential_name = SenderCredentialName::new(name.to_owned())
            .map_err(|error| CliError::InvalidInput(error.to_string()))?;
        let response = self
            .http
            .post(self.endpoint("/api/v1/authorization/sender")?)
            .json(&SenderAuthorizationBody {
                protocol_version: CURRENT_PROTOCOL_VERSION,
                credential_name,
            })
            .send()
            .map_err(CliError::Http)?;
        decode_json(response)
    }

    fn poll_authorization(
        &self,
        start: &AuthorizationStart,
    ) -> Result<PollingSecretClaimResult, CliError> {
        let body = PollRequest {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            request_id: &start.request.request_id,
            polling_secret: &start.polling_secret,
        };
        let response = self
            .http
            .post(self.endpoint("/api/v1/authorization/poll")?)
            .json(&body)
            .send()
            .map_err(CliError::Http)?;
        decode_json(response)
    }

    fn push(&self, bundle: Vec<u8>) -> Result<BundlePushResponse, CliError> {
        let response = self
            .sender_request(Method::PUT, "/api/v1/sender/bundle")?
            .header(reqwest::header::CONTENT_TYPE, "application/x-tar")
            .body(bundle)
            .send()
            .map_err(CliError::Http)?;
        decode_json(response)
    }

    fn clear(&self) -> Result<EmptyInboxResponse, CliError> {
        let response = self
            .sender_request(Method::DELETE, "/api/v1/sender/bundle")?
            .send()
            .map_err(CliError::Http)?;
        decode_json(response)
    }

    fn list_credentials(&self) -> Result<Vec<SenderCredentialMetadata>, CliError> {
        let response = self
            .sender_request(Method::GET, "/api/v1/sender/credentials")?
            .send()
            .map_err(CliError::Http)?;
        let response: CredentialListResponse = decode_json(response)?;
        Ok(response.credentials)
    }

    fn revoke(&self, name: &str) -> Result<CredentialRevokeResponse, CliError> {
        let name = SenderCredentialName::new(name.to_owned())
            .map_err(|error| CliError::InvalidInput(error.to_string()))?;
        let response = self
            .sender_request(Method::DELETE, "/api/v1/sender/credentials")?
            .json(&CredentialRevokeBody {
                protocol_version: CURRENT_PROTOCOL_VERSION,
                name,
            })
            .send()
            .map_err(CliError::Http)?;
        decode_json(response)
    }
}

pub fn run<I, S, Out, ErrOut>(args: I, stdout: Out, stderr: ErrOut) -> Result<(), CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
    Out: Write,
    ErrOut: Write,
{
    run_with_config(args, Config::from_environment()?, stdout, stderr)
}

pub fn run_with_config<I, S, Out, ErrOut>(
    args: I,
    config: Config,
    mut stdout: Out,
    mut stderr: ErrOut,
) -> Result<(), CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
    Out: Write,
    ErrOut: Write,
{
    let command = Command::parse(args)?;
    let client = ServiceClient::new(config)?;
    match command {
        Command::CredentialsCreate { name } => {
            writeln!(stderr, "creating sender credential `{name}`")?;
            let start = client.create_authorization(&name)?;
            writeln!(
                stderr,
                "open {} to approve the credential",
                start.request.approval_url
            )?;
            let grant = claim_sender(&client, &start, &mut stderr)?;
            writeln!(stderr, "sender credential `{name}` is ready")?;
            writeln!(stdout, "{}", grant.as_str())?;
        }
        Command::CredentialsList => {
            for credential in client.list_credentials()? {
                write_credential(&mut stdout, &credential)?;
            }
        }
        Command::CredentialsRevoke { name } => {
            let response = client.revoke(&name)?;
            if !response.revoked {
                return Err(CliError::Authorization(format!(
                    "service did not revoke credential `{}`",
                    response.name.as_str()
                )));
            }
            writeln!(
                stderr,
                "revoked sender credential `{}`",
                response.name.as_str()
            )?;
        }
        Command::Push { entry_point, files } => {
            let bundle = build_bundle(&entry_point, &files)?;
            let size = bundle.len();
            let response = client.push(bundle)?;
            writeln!(
                stderr,
                "published {size} bytes at revision {} (ETag {})",
                response.revision.value(),
                response.etag.as_str()
            )?;
            let _ = response.size_bytes;
        }
        Command::Clear => {
            let response = client.clear()?;
            writeln!(
                stderr,
                "cleared sender inbox at revision {} ({})",
                response.revision.value(),
                response.state
            )?;
        }
    }
    Ok(())
}

fn claim_sender<W: Write>(
    client: &ServiceClient,
    start: &AuthorizationStart,
    stderr: &mut W,
) -> Result<BearerToken, CliError> {
    loop {
        let result = client.poll_authorization(start)?;
        match result.outcome {
            PollingSecretClaimOutcome::Pending {
                retry_after_seconds,
            } => {
                writeln!(stderr, "waiting for human approval")?;
                thread::sleep(Duration::from_secs(u64::from(retry_after_seconds.max(1))));
            }
            PollingSecretClaimOutcome::Sender { credential } => return Ok(credential.bearer_token),
            PollingSecretClaimOutcome::Reader { .. } => {
                return Err(CliError::Authorization(
                    "service returned a reader session for a sender request".into(),
                ));
            }
            PollingSecretClaimOutcome::Denied => {
                return Err(CliError::Authorization(
                    "human approval denied the sender credential".into(),
                ));
            }
            PollingSecretClaimOutcome::Expired => {
                return Err(CliError::Authorization(
                    "sender authorization request expired".into(),
                ));
            }
            PollingSecretClaimOutcome::AlreadyClaimed => {
                return Err(CliError::Authorization(
                    "sender authorization request was already claimed".into(),
                ));
            }
        }
    }
}

fn build_bundle(entry_point: &str, files: &[String]) -> Result<Vec<u8>, CliError> {
    let mut builder = BundleBuilder::new(entry_point);
    builder.add_files(files);
    let mut bundle = Vec::new();
    builder.write(&mut bundle)?;
    Ok(bundle)
}

fn write_credential<W: Write>(
    output: &mut W,
    credential: &SenderCredentialMetadata,
) -> Result<(), CliError> {
    writeln!(
        output,
        "{}\t{}\tcreated_at={}\tlast_used_at={}\trevoked_at={}",
        credential.name.as_str(),
        credential.credential_id.as_str(),
        credential.created_at.value(),
        optional_timestamp(credential.last_used_at),
        optional_timestamp(credential.revoked_at)
    )?;
    Ok(())
}

fn optional_timestamp(timestamp: Option<Timestamp>) -> String {
    timestamp
        .map(|timestamp| timestamp.value().to_string())
        .unwrap_or_else(|| "-".into())
}

fn decode_json<T: DeserializeOwned>(response: Response) -> Result<T, CliError> {
    let status = response.status();
    let body = response.bytes().map_err(CliError::Http)?;
    if !status.is_success() {
        let error = serde_json::from_slice::<ApiError>(&body).ok();
        return Err(CliError::Server { status, error });
    }
    serde_json::from_slice(&body).map_err(CliError::Json)
}

fn missing_token() -> CliError {
    CliError::Configuration(format!("{TOKEN_ENV} must contain a sender bearer token"))
}

fn usage() -> String {
    format!(
        "usage:\n  prs-send credentials create --name NAME\n  prs-send credentials list\n  prs-send credentials revoke NAME\n  prs-send push ENTRYPOINT [FILE ...]\n  prs-send clear\n\nset {BASE_URL_ENV} and set {TOKEN_ENV} for commands that use an existing sender credential"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_parser_requires_a_name_for_creation() {
        assert!(Command::parse(["credentials", "create"]).is_err());
        assert!(Command::parse(["credentials", "create", "--name", "laptop"]).is_ok());
    }

    #[test]
    fn command_parser_keeps_all_explicit_push_files() {
        assert_eq!(
            Command::parse(["push", "docs/index.md", "docs/image.png"]).unwrap(),
            Command::Push {
                entry_point: "docs/index.md".into(),
                files: vec!["docs/image.png".into()]
            }
        );
    }

    #[test]
    fn list_output_contains_metadata_but_not_bearer_tokens() {
        let credential = SenderCredentialMetadata {
            credential_id: prs_sync_protocol::CredentialId::new("credential-1").unwrap(),
            name: SenderCredentialName::new("laptop").unwrap(),
            created_at: Timestamp::new(10),
            last_used_at: None,
            revoked_at: Some(Timestamp::new(20)),
            scope: prs_sync_protocol::SenderScope {
                capabilities: vec![prs_sync_protocol::SenderCapability::ManageCredentials],
            },
        };
        let mut output = Vec::new();
        write_credential(&mut output, &credential).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("laptop"));
        assert!(output.contains("credential-1"));
        assert!(!output.contains("bearer"));
    }

    #[test]
    fn bundle_builder_uses_the_explicit_input_set() {
        let root = unique_test_directory();
        std::fs::create_dir_all(root.join("docs/chapters")).unwrap();
        std::fs::write(root.join("docs/index.md"), "# Index").unwrap();
        std::fs::write(root.join("docs/chapters/one.md"), "# One").unwrap();
        std::fs::write(root.join("docs/ignored.md"), "# Ignored").unwrap();
        let bundle = build_bundle(
            &root.join("docs/index.md").to_string_lossy(),
            &[root
                .join("docs/chapters/one.md")
                .to_string_lossy()
                .into_owned()],
        )
        .unwrap();
        let manifest = prs_sync_bundle::validate(std::io::Cursor::new(bundle))
            .unwrap()
            .into_manifest();
        assert_eq!(manifest.entry_point.as_str(), "index.md");
        assert_eq!(manifest.files.len(), 2);
        assert!(!manifest
            .files
            .iter()
            .any(|file| file.path.as_str() == "ignored.md"));
        std::fs::remove_dir_all(root).unwrap();
    }

    fn unique_test_directory() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "prs-send-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(!std::path::Path::new(&path).exists());
        path
    }
}
