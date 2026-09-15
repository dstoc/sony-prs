use std::fmt;
use std::io;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    InvalidArgument(String),
    Protocol(String),
    Transport(scsi_transport::Error),
    Sony(sony_x50::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::InvalidArgument(message) => write!(f, "invalid argument: {message}"),
            Self::Protocol(message) => write!(f, "protocol error: {message}"),
            Self::Transport(error) => write!(f, "transport error: {error}"),
            Self::Sony(error) => write!(f, "Sony protocol error: {error}"),
        }
    }
}

impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<scsi_transport::Error> for Error {
    fn from(error: scsi_transport::Error) -> Self {
        Self::Transport(error)
    }
}

impl From<sony_x50::Error> for Error {
    fn from(error: sony_x50::Error) -> Self {
        Self::Sony(error)
    }
}
