use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    InvalidArgument(String),
    Protocol(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgument(message) => write!(f, "invalid argument: {message}"),
            Self::Protocol(message) => write!(f, "protocol error: {message}"),
        }
    }
}

impl std::error::Error for Error {}

pub const PREFIX: &str = "PRS1";
pub const MAX_LINE_LEN: usize = 1024;
pub const MAX_EXEC_BYTES: usize = 1024 * 1024;
pub const MAX_SHELL_BYTES: usize = 4096;
pub const MAX_SHELL_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Ping,
    Info,
    Status,
    Reboot,
    Probe,
    Render,
    Capture,
    Execute { bytes: u32, crc32: u32 },
    Shell { bytes: u32, crc32: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Pong,
    Info(String),
    Status(String),
    Rebooting,
    Probe(String),
    Rendered(String),
    Ready(String),
    Ack { kind: String, bytes: usize },
    Framebuffer(FramebufferHeader),
    Executed(String),
    Shell { bytes: usize, status: String },
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramebufferHeader {
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub bytes: usize,
}

pub fn encode_request(request: Request) -> Vec<u8> {
    let command = match request {
        Request::Ping => "PING",
        Request::Info => "INFO",
        Request::Status => "STATUS",
        Request::Reboot => "REBOOT",
        Request::Probe => "PROBE",
        Request::Render => "RENDER",
        Request::Capture => "CAPTURE",
        Request::Execute { bytes, crc32 } => {
            return format!("{PREFIX} EXEC {bytes} {crc32:08x}\n").into_bytes();
        }
        Request::Shell { bytes, crc32 } => {
            return format!("{PREFIX} SHELL {bytes} {crc32:08x}\n").into_bytes();
        }
    };
    format!("{PREFIX} {command}\n").into_bytes()
}

pub fn decode_request(line: &[u8]) -> Result<Request> {
    let line = decode_line(line)?;
    match line.as_str() {
        "PRS1 PING" => Ok(Request::Ping),
        "PRS1 INFO" => Ok(Request::Info),
        "PRS1 STATUS" => Ok(Request::Status),
        "PRS1 REBOOT" => Ok(Request::Reboot),
        "PRS1 PROBE" => Ok(Request::Probe),
        "PRS1 RENDER" => Ok(Request::Render),
        "PRS1 CAPTURE" => Ok(Request::Capture),
        _ if line.starts_with("PRS1 EXEC ") => parse_execute_request(&line),
        _ if line.starts_with("PRS1 SHELL ") => parse_shell_request(&line),
        _ => Err(Error::Protocol(format!(
            "unsupported serial request: {line:?}"
        ))),
    }
}

pub fn encode_response(response: &Response) -> Result<Vec<u8>> {
    let line = match response {
        Response::Pong => format!("{PREFIX} OK PONG"),
        Response::Info(value) => {
            validate_text(value, "serial info")?;
            format!("{PREFIX} OK INFO {value}")
        }
        Response::Status(value) => {
            validate_text(value, "serial status")?;
            format!("{PREFIX} OK STATUS {value}")
        }
        Response::Rebooting => format!("{PREFIX} OK REBOOTING"),
        Response::Probe(value) => {
            validate_text(value, "serial probe")?;
            format!("{PREFIX} OK PROBE {value}")
        }
        Response::Rendered(value) => {
            validate_text(value, "serial render")?;
            format!("{PREFIX} OK RENDERED {value}")
        }
        Response::Ready(value) => {
            validate_text(value, "serial ready")?;
            format!("{PREFIX} READY {value}")
        }
        Response::Ack { kind, bytes } => {
            validate_text(kind, "serial acknowledgement kind")?;
            format!("{PREFIX} ACK {kind} bytes={bytes}")
        }
        Response::Framebuffer(header) => encode_framebuffer_header(header)?,
        Response::Executed(value) => {
            validate_text(value, "serial exec")?;
            format!("{PREFIX} OK EXEC {value}")
        }
        Response::Shell { bytes, status } => {
            validate_text(status, "serial shell status")?;
            format!("{PREFIX} OK SHELL bytes={bytes} status={status}")
        }
        Response::Error(value) => {
            validate_text(value, "serial error")?;
            format!("{PREFIX} ERR {value}")
        }
    };
    Ok(format!("{line}\n").into_bytes())
}

pub fn decode_response(line: &[u8]) -> Result<Response> {
    let line = decode_line(line)?;
    if line == "PRS1 OK PONG" {
        return Ok(Response::Pong);
    }
    if let Some(value) = line.strip_prefix("PRS1 OK INFO ") {
        if value.is_empty() {
            return Err(Error::Protocol("serial info response is empty".into()));
        }
        return Ok(Response::Info(value.into()));
    }
    if let Some(value) = line.strip_prefix("PRS1 OK STATUS ") {
        if value.is_empty() {
            return Err(Error::Protocol("serial status response is empty".into()));
        }
        return Ok(Response::Status(value.into()));
    }
    if line == "PRS1 OK REBOOTING" {
        return Ok(Response::Rebooting);
    }
    if let Some(value) = line.strip_prefix("PRS1 OK PROBE ") {
        if value.is_empty() {
            return Err(Error::Protocol("serial probe response is empty".into()));
        }
        return Ok(Response::Probe(value.into()));
    }
    if let Some(value) = line.strip_prefix("PRS1 OK RENDERED ") {
        if value.is_empty() {
            return Err(Error::Protocol("serial render response is empty".into()));
        }
        return Ok(Response::Rendered(value.into()));
    }
    if let Some(value) = line.strip_prefix("PRS1 READY ") {
        if value.is_empty() {
            return Err(Error::Protocol("serial ready response is empty".into()));
        }
        return Ok(Response::Ready(value.into()));
    }
    if let Some(value) = line.strip_prefix("PRS1 ACK ") {
        return Ok(Response::Ack {
            kind: parse_ack_kind(value)?,
            bytes: parse_named_usize(value, "bytes")?,
        });
    }
    if line.starts_with("PRS1 OK FRAMEBUFFER ") {
        return Ok(Response::Framebuffer(parse_framebuffer_header(&line)?));
    }
    if let Some(value) = line.strip_prefix("PRS1 OK EXEC ") {
        if value.is_empty() {
            return Err(Error::Protocol("serial exec response is empty".into()));
        }
        return Ok(Response::Executed(value.into()));
    }
    if let Some(value) = line.strip_prefix("PRS1 OK SHELL ") {
        return Ok(Response::Shell {
            bytes: parse_named_usize(value, "bytes")?,
            status: parse_named_text(value, "status")?,
        });
    }
    if let Some(value) = line.strip_prefix("PRS1 ERR ") {
        if value.is_empty() {
            return Err(Error::Protocol("serial error response is empty".into()));
        }
        return Ok(Response::Error(value.into()));
    }
    Err(Error::Protocol(format!(
        "unsupported serial response: {line:?}"
    )))
}

fn parse_execute_request(line: &str) -> Result<Request> {
    let mut fields = line.split_whitespace();
    let prefix = fields.next();
    let command = fields.next();
    let bytes = fields
        .next()
        .ok_or_else(|| Error::Protocol("serial exec request is missing its byte count".into()))?
        .parse::<u32>()
        .map_err(|_| Error::Protocol("serial exec byte count is invalid".into()))?;
    let crc32 = u32::from_str_radix(
        fields
            .next()
            .ok_or_else(|| Error::Protocol("serial exec request is missing its CRC32".into()))?,
        16,
    )
    .map_err(|_| Error::Protocol("serial exec CRC32 is invalid".into()))?;
    if prefix != Some(PREFIX) || command != Some("EXEC") || fields.next().is_some() {
        return Err(Error::Protocol(format!(
            "invalid serial exec request: {line:?}"
        )));
    }
    if bytes == 0 || bytes as usize > MAX_EXEC_BYTES {
        return Err(Error::InvalidArgument(format!(
            "serial exec payload must be between 1 and {MAX_EXEC_BYTES} bytes"
        )));
    }
    Ok(Request::Execute { bytes, crc32 })
}

fn parse_shell_request(line: &str) -> Result<Request> {
    let mut fields = line.split_whitespace();
    if fields.next() != Some(PREFIX) || fields.next() != Some("SHELL") {
        return Err(Error::Protocol(format!(
            "invalid serial shell request: {line:?}"
        )));
    }
    let bytes = fields
        .next()
        .ok_or_else(|| Error::Protocol("serial shell request is missing its byte count".into()))?
        .parse::<u32>()
        .map_err(|_| Error::Protocol("serial shell byte count is invalid".into()))?;
    let crc32 = u32::from_str_radix(
        fields
            .next()
            .ok_or_else(|| Error::Protocol("serial shell request is missing its CRC32".into()))?,
        16,
    )
    .map_err(|_| Error::Protocol("serial shell CRC32 is invalid".into()))?;
    if fields.next().is_some() {
        return Err(Error::Protocol(format!(
            "invalid serial shell request: {line:?}"
        )));
    }
    if bytes == 0 || bytes as usize > MAX_SHELL_BYTES {
        return Err(Error::InvalidArgument(format!(
            "serial shell payload must be between 1 and {MAX_SHELL_BYTES} bytes"
        )));
    }
    Ok(Request::Shell { bytes, crc32 })
}

fn parse_named_usize(value: &str, name: &str) -> Result<usize> {
    let field = value
        .split_whitespace()
        .find(|field| field.starts_with(&format!("{name}=")))
        .ok_or_else(|| Error::Protocol(format!("serial shell response is missing {name}")))?;
    field
        .strip_prefix(&format!("{name}="))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| Error::Protocol(format!("serial shell {name} is invalid")))
}

fn parse_ack_kind(value: &str) -> Result<String> {
    let kind = value
        .split_whitespace()
        .next()
        .filter(|kind| !kind.is_empty())
        .ok_or_else(|| Error::Protocol("serial acknowledgement kind is missing".into()))?;
    Ok(kind.into())
}

fn parse_named_text(value: &str, name: &str) -> Result<String> {
    let field = value
        .split_whitespace()
        .find(|field| field.starts_with(&format!("{name}=")))
        .ok_or_else(|| Error::Protocol(format!("serial shell response is missing {name}")))?;
    let text = field
        .strip_prefix(&format!("{name}="))
        .filter(|text| !text.is_empty())
        .ok_or_else(|| Error::Protocol(format!("serial shell {name} is empty")))?;
    Ok(text.into())
}

fn encode_framebuffer_header(header: &FramebufferHeader) -> Result<String> {
    if header.width == 0 || header.height == 0 || header.bytes == 0 {
        return Err(Error::InvalidArgument(
            "framebuffer dimensions and byte count must be non-zero".into(),
        ));
    }
    validate_text(&header.format, "framebuffer format")?;
    let line = format!(
        "{PREFIX} OK FRAMEBUFFER width={} height={} format={} bytes={}",
        header.width, header.height, header.format, header.bytes
    );
    if line.len() + 1 > MAX_LINE_LEN {
        return Err(Error::InvalidArgument(
            "framebuffer header is too long".into(),
        ));
    }
    Ok(line)
}

fn parse_framebuffer_header(line: &str) -> Result<FramebufferHeader> {
    let mut fields = line.split_whitespace();
    if fields.next() != Some(PREFIX)
        || fields.next() != Some("OK")
        || fields.next() != Some("FRAMEBUFFER")
    {
        return Err(Error::Protocol(format!(
            "invalid framebuffer response: {line:?}"
        )));
    }
    let width = parse_header_field(&mut fields, "width")?;
    let height = parse_header_field(&mut fields, "height")?;
    let format = parse_header_text_field(&mut fields, "format")?;
    let bytes = parse_header_field::<usize>(&mut fields, "bytes")?;
    if fields.next().is_some() || width == 0 || height == 0 || bytes == 0 {
        return Err(Error::Protocol(
            "invalid framebuffer dimensions or byte count".into(),
        ));
    }
    Ok(FramebufferHeader {
        width,
        height,
        format,
        bytes,
    })
}

fn parse_header_field<'a, T>(fields: &mut impl Iterator<Item = &'a str>, name: &str) -> Result<T>
where
    T: std::str::FromStr,
{
    let field = fields
        .next()
        .ok_or_else(|| Error::Protocol(format!("framebuffer header is missing {name}")))?;
    let (key, value) = field
        .split_once('=')
        .ok_or_else(|| Error::Protocol(format!("framebuffer header field is invalid: {field}")))?;
    if key != name || value.is_empty() {
        return Err(Error::Protocol(format!(
            "framebuffer header expected {name}=..."
        )));
    }
    value
        .parse()
        .map_err(|_| Error::Protocol(format!("framebuffer header {name} is invalid")))
}

fn parse_header_text_field<'a>(
    fields: &mut impl Iterator<Item = &'a str>,
    name: &str,
) -> Result<String> {
    let field = fields
        .next()
        .ok_or_else(|| Error::Protocol(format!("framebuffer header is missing {name}")))?;
    let (key, value) = field
        .split_once('=')
        .ok_or_else(|| Error::Protocol(format!("framebuffer header field is invalid: {field}")))?;
    if key != name || value.is_empty() {
        return Err(Error::Protocol(format!(
            "framebuffer header expected {name}=..."
        )));
    }
    Ok(value.into())
}

fn decode_line(line: &[u8]) -> Result<String> {
    if line.len() > MAX_LINE_LEN {
        return Err(Error::Protocol(format!(
            "serial line is {} bytes, maximum is {MAX_LINE_LEN}",
            line.len()
        )));
    }
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    String::from_utf8(line.to_vec())
        .map_err(|_| Error::Protocol("serial line is not valid UTF-8".into()))
}

fn validate_text(value: &str, field: &str) -> Result<()> {
    if value.is_empty() || value.bytes().any(|byte| byte == b'\r' || byte == b'\n') {
        return Err(Error::InvalidArgument(format!(
            "{field} must be non-empty and cannot contain a newline"
        )));
    }
    if format!("{PREFIX} ERR {value}").len() > MAX_LINE_LEN {
        return Err(Error::InvalidArgument(format!("{field} is too long")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_round_trips() {
        let request = encode_request(Request::Ping);
        assert_eq!(request, b"PRS1 PING\n");
        assert_eq!(decode_request(&request).unwrap(), Request::Ping);

        let response = encode_response(&Response::Pong).unwrap();
        assert_eq!(response, b"PRS1 OK PONG\n");
        assert_eq!(decode_response(&response).unwrap(), Response::Pong);
    }

    #[test]
    fn info_response_preserves_spaces() {
        let response = Response::Info("model=PRS-350 transport=cdc-acm".into());
        let bytes = encode_response(&response).unwrap();
        assert_eq!(
            decode_response(&bytes).unwrap(),
            Response::Info("model=PRS-350 transport=cdc-acm".into())
        );
    }

    #[test]
    fn status_round_trips() {
        let request = encode_request(Request::Status);
        assert_eq!(request, b"PRS1 STATUS\n");
        assert_eq!(decode_request(&request).unwrap(), Request::Status);

        let response = Response::Status("ui=stock-alive".into());
        let bytes = encode_response(&response).unwrap();
        assert_eq!(bytes, b"PRS1 OK STATUS ui=stock-alive\n");
        assert_eq!(decode_response(&bytes).unwrap(), response);
    }

    #[test]
    fn reboot_round_trips() {
        let request = encode_request(Request::Reboot);
        assert_eq!(request, b"PRS1 REBOOT\n");
        assert_eq!(decode_request(&request).unwrap(), Request::Reboot);

        let response = encode_response(&Response::Rebooting).unwrap();
        assert_eq!(response, b"PRS1 OK REBOOTING\n");
        assert_eq!(decode_response(&response).unwrap(), Response::Rebooting);
    }

    #[test]
    fn probe_round_trips() {
        let request = encode_request(Request::Probe);
        assert_eq!(request, b"PRS1 PROBE\n");
        assert_eq!(decode_request(&request).unwrap(), Request::Probe);

        let response = Response::Probe("fb=600x800-bpp=8 input=/dev/input/event0".into());
        let bytes = encode_response(&response).unwrap();
        assert_eq!(decode_response(&bytes).unwrap(), response);
    }

    #[test]
    fn render_round_trips() {
        let request = encode_request(Request::Render);
        assert_eq!(request, b"PRS1 RENDER\n");
        assert_eq!(decode_request(&request).unwrap(), Request::Render);

        let response = Response::Rendered("600x800-checkerboard".into());
        let bytes = encode_response(&response).unwrap();
        assert_eq!(decode_response(&bytes).unwrap(), response);
    }

    #[test]
    fn capture_round_trips() {
        let request = encode_request(Request::Capture);
        assert_eq!(request, b"PRS1 CAPTURE\n");
        assert_eq!(decode_request(&request).unwrap(), Request::Capture);

        let response = Response::Framebuffer(FramebufferHeader {
            width: 600,
            height: 800,
            format: "gray8".into(),
            bytes: 480_000,
        });
        let bytes = encode_response(&response).unwrap();
        assert_eq!(
            bytes,
            b"PRS1 OK FRAMEBUFFER width=600 height=800 format=gray8 bytes=480000\n"
        );
        assert_eq!(decode_response(&bytes).unwrap(), response);
    }

    #[test]
    fn execute_round_trips() {
        let request = Request::Execute {
            bytes: 1234,
            crc32: 0xdead_beef,
        };
        let bytes = encode_request(request);
        assert_eq!(bytes, b"PRS1 EXEC 1234 deadbeef\n");
        assert_eq!(decode_request(&bytes).unwrap(), request);

        let response = Response::Executed("size=1234 crc32=deadbeef status=0".into());
        let bytes = encode_response(&response).unwrap();
        assert_eq!(decode_response(&bytes).unwrap(), response);

        let ready = encode_response(&Response::Ready("EXEC".into())).unwrap();
        assert_eq!(ready, b"PRS1 READY EXEC\n");
        assert_eq!(
            decode_response(&ready).unwrap(),
            Response::Ready("EXEC".into())
        );

        let ack = encode_response(&Response::Ack {
            kind: "EXEC".into(),
            bytes: 1024,
        })
        .unwrap();
        assert_eq!(ack, b"PRS1 ACK EXEC bytes=1024\n");
        assert_eq!(
            decode_response(&ack).unwrap(),
            Response::Ack {
                kind: "EXEC".into(),
                bytes: 1024,
            }
        );
    }

    #[test]
    fn shell_round_trips() {
        let request = Request::Shell {
            bytes: 16,
            crc32: 0x1234_5678,
        };
        let bytes = encode_request(request);
        assert_eq!(bytes, b"PRS1 SHELL 16 12345678\n");
        assert_eq!(decode_request(&bytes).unwrap(), request);

        let response = Response::Shell {
            bytes: 12,
            status: "exit=0".into(),
        };
        let bytes = encode_response(&response).unwrap();
        assert_eq!(bytes, b"PRS1 OK SHELL bytes=12 status=exit=0\n");
        assert_eq!(decode_response(&bytes).unwrap(), response);
    }

    #[test]
    fn malformed_or_mutating_messages_are_rejected() {
        assert!(decode_request(b"PRS1 WRITE\n").is_err());
        assert!(decode_request(b"PRS1 EXEC 0 00000000\n").is_err());
        assert!(decode_request(b"PRS1 EXEC 10 nope\n").is_err());
        assert!(decode_request(b"PRS1 SHELL 0 00000000\n").is_err());
        assert!(decode_response(b"PRS1 OK\n").is_err());
        assert!(encode_response(&Response::Error("bad\nvalue".into())).is_err());
    }
}
