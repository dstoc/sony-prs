use crate::error::{Error, Result};
use crate::serial_protocol::{self, Request, Response, MAX_LINE_LEN};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);

pub struct SerialClient {
    path: PathBuf,
    reader: File,
    writer: File,
    timeout: Duration,
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

    fn exchange(&mut self, request: Request) -> Result<Response> {
        self.writer
            .write_all(&serial_protocol::encode_request(request))?;
        self.writer.flush()?;

        let deadline = Instant::now() + self.timeout;
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
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
                        return serial_protocol::decode_response(&line);
                    }
                }
                Ok(_) => unreachable!(),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        }
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
