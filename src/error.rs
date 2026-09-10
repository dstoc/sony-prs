use std::fmt;
use std::io;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    InvalidArgument(String),
    Protocol(String),
    Scsi(ScsiError),
    Unsupported(String),
}

#[derive(Debug, Clone)]
pub struct ScsiError {
    pub status: u8,
    pub host_status: u16,
    pub driver_status: u16,
    pub sense: Vec<u8>,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::InvalidArgument(message) => write!(f, "invalid argument: {message}"),
            Self::Protocol(message) => write!(f, "protocol error: {message}"),
            Self::Scsi(error) => write!(f, "SCSI error: {error}"),
            Self::Unsupported(message) => write!(f, "unsupported: {message}"),
        }
    }
}

impl fmt::Display for ScsiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "status=0x{:02x}, host=0x{:04x}, driver=0x{:04x}",
            self.status, self.host_status, self.driver_status
        )?;
        if !self.sense.is_empty() {
            write!(f, ", sense={}", hex(&self.sense))?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}
impl std::error::Error for ScsiError {}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}
