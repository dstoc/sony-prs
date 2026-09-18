use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use prs_send::{run_with_config, Config};

#[test]
fn create_prints_the_claimed_token_only_to_stdout() {
    let (base_url, requests, server) = mock_server(vec![
        (
            200,
            r#"{"protocol_version":{"major":1,"minor":0},"request":{"protocol_version":{"major":1,"minor":0},"request_id":"auth-1","kind":"sender","credential_name":"laptop","approval_url":"https://reader.example/a/auth-1","created_at":100,"expires_at":160},"polling_secret":"poll-secret"}"#
                .into(),
        ),
        (
            200,
            r#"{"protocol_version":{"major":1,"minor":0},"outcome":{"kind":"sender","credential":{"bearer_token":"sender-secret","metadata":{"credential_id":"credential-1","name":"laptop","created_at":100,"scope":{"capabilities":["upload_bundle","clear_inbox","manage_credentials"]}}}}}"#
                .into(),
        ),
    ]);
    let (stdout, stderr, result) = run(
        ["credentials", "create", "--name", "laptop"],
        Config {
            base_url,
            sender_token: None,
        },
    );
    assert!(result.is_ok(), "create failed: {result:?}");
    assert_eq!(stdout, "sender-secret\n");
    assert!(stderr.contains("https://reader.example/a/auth-1"));
    assert!(!stderr.contains("sender-secret"));

    let start_request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(start_request.method, "POST");
    assert_eq!(start_request.path, "/api/v1/authorization/sender");
    assert!(String::from_utf8_lossy(&start_request.body).contains("laptop"));
    assert!(start_request.header("authorization").is_none());
    let poll_request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(poll_request.method, "POST");
    assert_eq!(poll_request.path, "/api/v1/authorization/poll");
    assert!(String::from_utf8_lossy(&poll_request.body).contains("auth-1"));
    assert!(String::from_utf8_lossy(&poll_request.body).contains("poll-secret"));
    server.join().unwrap();
}

#[test]
fn create_ignores_an_invalid_sender_token() {
    let (base_url, requests, server) = mock_server(vec![
        (
            200,
            r#"{"protocol_version":{"major":1,"minor":0},"request":{"protocol_version":{"major":1,"minor":0},"request_id":"auth-1","kind":"sender","credential_name":"laptop","approval_url":"https://reader.example/a/auth-1","created_at":100,"expires_at":160},"polling_secret":"poll-secret"}"#
                .into(),
        ),
        (
            200,
            r#"{"protocol_version":{"major":1,"minor":0},"outcome":{"kind":"sender","credential":{"bearer_token":"sender-secret","metadata":{"credential_id":"credential-1","name":"laptop","created_at":100,"scope":{"capabilities":["upload_bundle","clear_inbox","manage_credentials"]}}}}}"#
                .into(),
        ),
    ]);
    let (stdout, stderr, result) = run(
        ["credentials", "create", "--name", "laptop"],
        Config {
            base_url,
            sender_token: Some("bad token".into()),
        },
    );
    assert!(result.is_ok(), "create failed: {result:?}");
    assert_eq!(stdout, "sender-secret\n");
    assert!(stderr.contains("https://reader.example/a/auth-1"));
    assert!(!stderr.contains("sender-secret"));

    let start_request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(start_request.method, "POST");
    assert_eq!(start_request.path, "/api/v1/authorization/sender");
    assert!(start_request.header("authorization").is_none());
    let poll_request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(poll_request.method, "POST");
    assert_eq!(poll_request.path, "/api/v1/authorization/poll");
    assert!(poll_request.header("authorization").is_none());
    server.join().unwrap();
}

#[test]
fn create_surfaces_an_active_duplicate_name_as_a_server_conflict() {
    let (base_url, requests, server) = mock_server(vec![
        (
            409,
            r#"{"protocol_version":{"major":1,"minor":0},"error":{"code":"conflict","message":"an active sender credential already uses this name"}}"#
                .into(),
        ),
    ]);
    let (stdout, stderr, result) = run(
        ["credentials", "create", "--name", "laptop"],
        Config {
            base_url,
            sender_token: None,
        },
    );
    assert!(stdout.is_empty());
    assert!(stderr.contains("creating sender credential"));
    assert!(result.unwrap_err().to_string().contains("HTTP 409"));
    let request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(request.path, "/api/v1/authorization/sender");
    server.join().unwrap();
}

#[test]
fn clear_accepts_an_empty_inbox_using_the_sender_route() {
    let (base_url, requests, server) = mock_server(vec![(
        200,
        r#"{"protocol_version":{"major":1,"minor":0},"revision":0,"state":"empty"}"#.into(),
    )]);
    let (stdout, stderr, result) = run(
        ["clear"],
        Config {
            base_url,
            sender_token: Some("sender-token".into()),
        },
    );
    assert!(result.is_ok(), "clear failed: {result:?}");
    assert!(stdout.is_empty());
    assert!(stderr.contains("cleared sender inbox at revision 0 (empty)"));

    let request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(request.method, "DELETE");
    assert_eq!(request.path, "/api/v1/sender/bundle");
    assert_eq!(request.header("authorization"), Some("Bearer sender-token"));
    server.join().unwrap();
}

#[test]
fn list_surfaces_a_server_failure_without_reading_bundle_content() {
    let (base_url, requests, server) = mock_server(vec![
        (
            500,
            r#"{"protocol_version":{"major":1,"minor":0},"error":{"code":"internal","message":"database unavailable"}}"#
                .into(),
        ),
    ]);
    let (stdout, stderr, result) = run(
        ["credentials", "list"],
        Config {
            base_url,
            sender_token: Some("sender-token".into()),
        },
    );
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
    let error = result.unwrap_err().to_string();
    assert!(error.contains("HTTP 500"));
    assert!(error.contains("database unavailable"));
    assert!(!error.contains("reader"));

    let request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/api/v1/sender/credentials");
    assert_eq!(request.header("authorization"), Some("Bearer sender-token"));
    server.join().unwrap();
}

#[test]
fn push_sends_only_the_explicit_bundle_and_never_reads_the_inbox() {
    let root = unique_directory();
    std::fs::create_dir_all(root.join("docs/chapters")).unwrap();
    std::fs::write(root.join("docs/index.md"), "# Index").unwrap();
    std::fs::write(root.join("docs/chapters/one.md"), "# One").unwrap();
    std::fs::write(root.join("docs/ignored.md"), "# Ignored").unwrap();
    let entry_point = root.join("docs/index.md");
    let additional = root.join("docs/chapters/one.md");

    let (base_url, requests, server) = mock_server(vec![
        (
            200,
            r#"{"protocol_version":{"major":1,"minor":0},"revision":4,"etag":"etag-4","size_bytes":256}"#
                .into(),
        ),
    ]);
    let (stdout, stderr, result) = run(
        [
            "push",
            entry_point.to_str().unwrap(),
            additional.to_str().unwrap(),
        ],
        Config {
            base_url,
            sender_token: Some("sender-token".into()),
        },
    );
    assert!(result.is_ok(), "push failed: {result:?}");
    assert!(stdout.is_empty());
    assert!(stderr.contains("published"));

    let request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(request.method, "PUT");
    assert_eq!(request.path, "/api/v1/sender/bundle");
    assert_eq!(request.header("authorization"), Some("Bearer sender-token"));
    let manifest = prs_sync_bundle::validate(std::io::Cursor::new(request.body))
        .unwrap()
        .into_manifest();
    assert_eq!(manifest.entry_point.as_str(), "index.md");
    assert_eq!(
        manifest
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        vec!["index.md", "chapters/one.md"]
    );
    server.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

fn run<I, S>(args: I, config: Config) -> (String, String, Result<(), prs_send::CliError>)
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = run_with_config(args, config, &mut stdout, &mut stderr);
    (
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
        result,
    )
}

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

fn mock_server(responses: Vec<(u16, String)>) -> (String, Receiver<Request>, JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = mpsc::channel();
    let server = thread::spawn(move || {
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            sender.send(request).unwrap();
            let reason = match status {
                200 => "OK",
                500 => "Internal Server Error",
                _ => "Response",
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (format!("http://{address}"), receiver, server)
}

fn read_request(stream: &mut TcpStream) -> Request {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0u8; 4096];
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0, "client closed before sending headers");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let header_text = String::from_utf8_lossy(&bytes[..header_end]);
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().unwrap();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap().to_owned();
    let path = request_parts.next().unwrap().to_owned();
    let headers = lines
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            Some((key.to_owned(), value.trim().to_owned()))
        })
        .collect::<Vec<_>>();
    let content_length = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.parse::<usize>().unwrap())
        .unwrap_or(0);
    while bytes.len() - header_end < content_length {
        let mut chunk = [0u8; 4096];
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0, "client closed before sending the body");
        bytes.extend_from_slice(&chunk[..read]);
    }
    Request {
        method,
        path,
        headers,
        body: bytes[header_end..header_end + content_length].to_vec(),
    }
}

fn unique_directory() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "prs-send-cli-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    assert!(!path.exists());
    path
}
