//! Read-only protocol building blocks for Sony PRS-x50 extraction.
//!
//! The crate deliberately does not expose write, delete, update-mode, or
//! arbitrary-command operations. The x50 SCSI framing and read-only file
//! service are isolated in [`sony::SonyExtendedTransport`].

pub mod client;
pub mod error;
pub mod protocol;
pub mod serial;
pub mod serial_protocol;
pub mod sg;
pub mod sony;

pub use client::{PacketTransport, ReaderClient};
pub use error::{Error, Result};
pub use sony::SonyExtendedTransport;
