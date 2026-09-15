use std::fmt;
use std::io;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    InvalidArgument(String),
    Protocol(String),
    Unsupported(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::InvalidArgument(message) => write!(f, "invalid argument: {message}"),
            Self::Protocol(message) => write!(f, "protocol error: {message}"),
            Self::Unsupported(message) => write!(f, "unsupported: {message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<prs350_wire::Error> for Error {
    fn from(error: prs350_wire::Error) -> Self {
        match error {
            prs350_wire::Error::InvalidArgument(message) => Self::InvalidArgument(message),
            prs350_wire::Error::Protocol(message) => Self::Protocol(message),
        }
    }
}
