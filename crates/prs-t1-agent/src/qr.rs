//! Small, allocation-bounded QR matrix wrapper for the T1 authorization view.

use qrcode::{types::Color, QrCode};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QrMatrix {
    size: usize,
    modules: Vec<bool>,
}

impl QrMatrix {
    pub(crate) fn encode(value: &str) -> Result<Self, QrError> {
        if value.is_empty() {
            return Err(QrError::EmptyValue);
        }
        let code = QrCode::new(value.as_bytes()).map_err(|_| QrError::TooLarge)?;
        let size = code.width();
        let mut modules = Vec::with_capacity(size.saturating_mul(size));
        for y in 0..size {
            for x in 0..size {
                modules.push(code[(x, y)] == Color::Dark);
            }
        }
        Ok(Self { size, modules })
    }

    pub(crate) const fn size(&self) -> usize {
        self.size
    }

    pub(crate) fn is_dark(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QrError {
    EmptyValue,
    TooLarge,
}

impl fmt::Display for QrError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyValue => formatter.write_str("QR value must not be empty"),
            Self::TooLarge => formatter.write_str("approval URL is too large for a QR code"),
        }
    }
}

impl std::error::Error for QrError {}

#[cfg(test)]
mod tests {
    use super::QrMatrix;

    #[test]
    fn encodes_public_approval_url() {
        let qr = QrMatrix::encode("https://reader.example/a/request-1").unwrap();
        assert!(qr.size() >= 21);
        assert_eq!(qr.modules.len(), qr.size() * qr.size());
        assert!(qr.modules.iter().any(|dark| *dark));
    }

    #[test]
    fn rejects_empty_value() {
        assert!(QrMatrix::encode("").is_err());
    }
}
