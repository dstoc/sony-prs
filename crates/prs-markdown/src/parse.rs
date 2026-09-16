//! Boundary between Markdown parser events and the owned document IR.

use crate::document::Document;
use std::error::Error;
use std::fmt;

pub trait MarkdownParser {
    fn parse(&self, source: &str) -> Result<Document, ParseError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    message: String,
}

impl ParseError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ParseError {}

/// Placeholder used until a concrete CommonMark/GFM parser is selected.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnsupportedParser;

impl MarkdownParser for UnsupportedParser {
    fn parse(&self, _source: &str) -> Result<Document, ParseError> {
        Err(ParseError::new(
            "Markdown parsing is not implemented by the architecture crate",
        ))
    }
}
