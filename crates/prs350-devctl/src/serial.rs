use crate::error::{Error, Result};
use prs350_wire::{self, FramebufferHeader, Request, Response, MAX_LINE_LEN};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);
const UPLOAD_CHUNK_SIZE: usize = 1024;

pub struct SerialClient {
    path: PathBuf,
    reader: File,
    writer: File,
    timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellResult {
    pub status: String,
    pub output: Vec<u8>,
}

impl SerialClient {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        configure(&path)?;
        let writer = OpenOptions::new().read(true).write(true).open(&path)?;
        let reader = writer.try_clone()?;
        Ok(Self {
            path,
            reader,
            writer,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn ping(&mut self) -> Result<()> {
        match self.exchange(Request::Ping)? {
            Response::Pong => Ok(()),
            Response::Error(message) => {
                Err(Error::Protocol(format!("reader rejected ping: {message}")))
            }
            response => Err(Error::Protocol(format!(
                "unexpected ping response: {response:?}"
            ))),
        }
    }

    pub fn info(&mut self) -> Result<String> {
        match self.exchange(Request::Info)? {
            Response::Info(value) => Ok(value),
            Response::Error(message) => {
                Err(Error::Protocol(format!("reader rejected info: {message}")))
            }
            response => Err(Error::Protocol(format!(
                "unexpected info response: {response:?}"
            ))),
        }
    }

    pub fn status(&mut self) -> Result<String> {
        match self.exchange(Request::Status)? {
            Response::Status(value) => Ok(value),
            Response::Error(message) => Err(Error::Protocol(format!(
                "reader rejected status: {message}"
            ))),
            response => Err(Error::Protocol(format!(
                "unexpected status response: {response:?}"
            ))),
        }
    }

    pub fn reboot(&mut self) -> Result<()> {
        match self.exchange(Request::Reboot)? {
            Response::Rebooting => Ok(()),
            Response::Error(message) => Err(Error::Protocol(format!(
                "reader rejected reboot: {message}"
            ))),
            response => Err(Error::Protocol(format!(
                "unexpected reboot response: {response:?}"
            ))),
        }
    }

    pub fn probe(&mut self) -> Result<String> {
        match self.exchange(Request::Probe)? {
            Response::Probe(value) => Ok(value),
            Response::Error(message) => {
                Err(Error::Protocol(format!("reader rejected probe: {message}")))
            }
            response => Err(Error::Protocol(format!(
                "unexpected probe response: {response:?}"
            ))),
        }
    }

    pub fn render(&mut self) -> Result<String> {
        match self.exchange(Request::Render)? {
            Response::Rendered(value) => Ok(value),
            Response::Error(message) => Err(Error::Protocol(format!(
                "reader rejected render: {message}"
            ))),
            response => Err(Error::Protocol(format!(
                "unexpected render response: {response:?}"
            ))),
        }
    }

    pub fn screenshot(&mut self, output_path: impl AsRef<Path>) -> Result<FramebufferHeader> {
        self.writer
            .write_all(&prs350_wire::encode_request(Request::Capture))?;
        self.writer.flush()?;

        let response = self.read_response_line()?;
        let header = match response {
            Response::Framebuffer(header) => header,
            Response::Error(message) => {
                return Err(Error::Protocol(format!(
                    "reader rejected framebuffer capture: {message}"
                )))
            }
            response => {
                return Err(Error::Protocol(format!(
                    "unexpected framebuffer response: {response:?}"
                )))
            }
        };
        if header.format != "gray8" {
            return Err(Error::Unsupported(format!(
                "reader framebuffer format is {}, expected gray8",
                header.format
            )));
        }
        let expected_bytes = header.width as usize * header.height as usize;
        if header.bytes != expected_bytes {
            return Err(Error::Protocol(format!(
                "reader framebuffer byte count is {}, expected {expected_bytes}",
                header.bytes
            )));
        }

        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output_path)?;
        writeln!(output, "P5")?;
        writeln!(output, "{} {}", header.width, header.height)?;
        writeln!(output, "255")?;
        self.read_exact_with_deadline(&mut output, header.bytes)?;
        Ok(header)
    }

    pub fn execute(&mut self, binary_path: impl AsRef<Path>) -> Result<String> {
        let binary_path = binary_path.as_ref();
        let data = fs::read(binary_path)?;
        if data.is_empty() || data.len() > prs350_wire::MAX_EXEC_BYTES {
            return Err(Error::InvalidArgument(format!(
                "ARM binary must be between 1 and {} bytes",
                prs350_wire::MAX_EXEC_BYTES
            )));
        }
        let crc32 = crc32(&data);
        self.writer
            .write_all(&prs350_wire::encode_request(Request::Execute {
                bytes: data.len() as u32,
                crc32,
            }))?;
        self.writer.flush()?;

        match self.read_response_line()? {
            Response::Ready(value) if value == "EXEC" => {}
            Response::Error(message) => {
                return Err(Error::Protocol(format!(
                    "reader rejected exec setup: {message}"
                )))
            }
            response => {
                return Err(Error::Protocol(format!(
                    "unexpected exec setup response: {response:?}"
                )))
            }
        }

        self.write_payload(&data, "EXEC")?;

        match self.read_response_line()? {
            Response::Executed(value) => Ok(value),
            Response::Error(message) => Err(Error::Protocol(format!(
                "reader rejected execution: {message}"
            ))),
            response => Err(Error::Protocol(format!(
                "unexpected exec response: {response:?}"
            ))),
        }
    }

    pub fn shell(&mut self, command: &str) -> Result<ShellResult> {
        let data = command.as_bytes();
        if data.is_empty() || data.len() > prs350_wire::MAX_SHELL_BYTES {
            return Err(Error::InvalidArgument(format!(
                "shell command must be between 1 and {} bytes",
                prs350_wire::MAX_SHELL_BYTES
            )));
        }
        let crc32 = crc32(data);
        self.writer
            .write_all(&prs350_wire::encode_request(Request::Shell {
                bytes: data.len() as u32,
                crc32,
            }))?;
        self.writer.flush()?;

        match self.read_response_line()? {
            Response::Ready(value) if value == "SHELL" => {}
            Response::Error(message) => {
                return Err(Error::Protocol(format!(
                    "reader rejected shell setup: {message}"
                )))
            }
            response => {
                return Err(Error::Protocol(format!(
                    "unexpected shell setup response: {response:?}"
                )))
            }
        }

        self.write_payload(data, "SHELL")?;
        let response = self.read_response_line()?;
        let (bytes, status) = match response {
            Response::Shell { bytes, status } => (bytes, status),
            Response::Error(message) => {
                return Err(Error::Protocol(format!(
                    "reader rejected shell command: {message}"
                )))
            }
            response => {
                return Err(Error::Protocol(format!(
                    "unexpected shell response: {response:?}"
                )))
            }
        };
        if bytes > prs350_wire::MAX_SHELL_OUTPUT_BYTES {
            return Err(Error::Protocol(format!(
                "reader shell output is {bytes} bytes, maximum is {}",
                prs350_wire::MAX_SHELL_OUTPUT_BYTES
            )));
        }
        let output = self.read_exact_bytes(bytes)?;
        Ok(ShellResult { status, output })
    }

    fn exchange(&mut self, request: Request) -> Result<Response> {
        self.writer
            .write_all(&prs350_wire::encode_request(request))?;
        self.writer.flush()?;

        self.read_response_line()
    }

    fn read_response_line(&mut self) -> Result<Response> {
        let line = self.read_line()?;
        prs350_wire::decode_response(&line).map_err(Into::into)
    }

    fn write_payload(&mut self, data: &[u8], kind: &str) -> Result<()> {
        let mut acknowledged = 0usize;
        for chunk in data.chunks(UPLOAD_CHUNK_SIZE) {
            self.writer.write_all(chunk)?;
            self.writer.flush()?;
            match self.read_response_line()? {
                Response::Ack {
                    kind: ack_kind,
                    bytes,
                } if ack_kind == kind && bytes == acknowledged + chunk.len() => {
                    acknowledged = bytes;
                }
                Response::Error(message) => {
                    return Err(Error::Protocol(format!(
                        "reader rejected {kind} upload: {message}"
                    )))
                }
                response => {
                    return Err(Error::Protocol(format!(
                        "unexpected {kind} upload acknowledgement: {response:?}"
                    )))
                }
            }
        }
        Ok(())
    }

    fn read_line(&mut self) -> Result<Vec<u8>> {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        let deadline = Instant::now() + self.timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(Error::Protocol(format!(
                    "timed out waiting for a response from {}",
                    self.path.display()
                )));
            }
            match self.reader.read(&mut byte) {
                Ok(0) => continue,
                Ok(1) => {
                    line.push(byte[0]);
                    if line.len() > MAX_LINE_LEN {
                        return Err(Error::Protocol(format!(
                            "serial response exceeds {MAX_LINE_LEN} bytes"
                        )));
                    }
                    if byte[0] == b'\n' {
                        return Ok(line);
                    }
                }
                Ok(_) => unreachable!(),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn read_exact_with_deadline(&mut self, output: &mut File, length: usize) -> Result<()> {
        let data = self.read_exact_bytes(length)?;
        output.write_all(&data)?;
        Ok(())
    }

    fn read_exact_bytes(&mut self, length: usize) -> Result<Vec<u8>> {
        let deadline = Instant::now() + self.timeout;
        let mut remaining = length;
        let mut buffer = [0u8; 16 * 1024];
        let mut output = Vec::with_capacity(length);
        while remaining != 0 {
            if Instant::now() >= deadline {
                return Err(Error::Protocol(format!(
                    "timed out waiting for framebuffer data from {}",
                    self.path.display()
                )));
            }
            let requested = remaining.min(buffer.len());
            match self.reader.read(&mut buffer[..requested]) {
                Ok(0) => continue,
                Ok(bytes) => {
                    output.extend_from_slice(&buffer[..bytes]);
                    remaining -= bytes;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Ok(output)
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::crc32;

    #[test]
    fn crc32_matches_standard_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }
}

fn configure(path: &Path) -> Result<()> {
    let status = Command::new("stty")
        .args([
            "-F",
            path.to_str()
                .ok_or_else(|| Error::InvalidArgument("serial path is not UTF-8".into()))?,
            "9600",
            "raw",
            "-echo",
            "-ixon",
            "-ixoff",
            "min",
            "0",
            "time",
            "10",
        ])
        .status()?;
    if !status.success() {
        return Err(Error::Unsupported(format!(
            "stty could not configure {}",
            path.display()
        )));
    }
    Ok(())
}
