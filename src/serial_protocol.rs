use crate::error::{Error, Result};

pub const PREFIX: &str = "PRS1";
pub const MAX_LINE_LEN: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Ping,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Pong,
    Info(String),
    Error(String),
}

pub fn encode_request(request: Request) -> Vec<u8> {
    let command = match request {
        Request::Ping => "PING",
        Request::Info => "INFO",
    };
    format!("{PREFIX} {command}\n").into_bytes()
}

pub fn decode_request(line: &[u8]) -> Result<Request> {
    let line = decode_line(line)?;
    match line.as_str() {
        "PRS1 PING" => Ok(Request::Ping),
        "PRS1 INFO" => Ok(Request::Info),
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
    fn malformed_or_mutating_messages_are_rejected() {
        assert!(decode_request(b"PRS1 WRITE\n").is_err());
        assert!(decode_response(b"PRS1 OK\n").is_err());
        assert!(encode_response(&Response::Error("bad\nvalue".into())).is_err());
    }
}
