//! Document and anchor navigation, independent of physical input.

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DocumentId(String);

impl DocumentId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl AsRef<str> for DocumentId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for DocumentId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for DocumentId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DocumentLocation {
    pub document: DocumentId,
    pub anchor: Option<String>,
}

impl DocumentLocation {
    pub fn new(document: DocumentId, anchor: Option<String>) -> Self {
        Self { document, anchor }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NavigationTarget {
    Location(DocumentLocation),
    Anchor(String),
    Document(DocumentId),
    DocumentAnchor {
        document: DocumentId,
        anchor: String,
    },
    Asset(DocumentId),
    External(String),
}

impl NavigationTarget {
    pub fn from_destination(destination: &str) -> Self {
        if destination.starts_with('#') {
            return Self::Anchor(destination.trim_start_matches('#').to_owned());
        }
        if is_external_destination(destination) {
            return Self::External(destination.to_owned());
        }

        let (document, anchor) = destination
            .split_once('#')
            .map(|(document, anchor)| (document, Some(anchor.to_owned())))
            .unwrap_or((destination, None));
        let document = DocumentId::from(document);
        if !is_markdown_destination(document.as_ref()) {
            return Self::Asset(document);
        }
        match anchor {
            Some(anchor) => Self::DocumentAnchor { document, anchor },
            None => Self::Document(document),
        }
    }
}

fn is_markdown_destination(destination: &str) -> bool {
    destination.rsplit_once('.').is_some_and(|(_, extension)| {
        extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
    })
}

fn is_external_destination(destination: &str) -> bool {
    if destination.starts_with("//") {
        return true;
    }

    let Some(colon) = destination.find(':') else {
        return false;
    };
    let scheme = &destination[..colon];
    !scheme.is_empty()
        && scheme.chars().enumerate().all(|(index, character)| {
            if index == 0 {
                character.is_ascii_alphabetic()
            } else {
                character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations_keep_document_anchor_asset_and_external_kinds_distinct() {
        assert_eq!(
            NavigationTarget::from_destination("chapter.md"),
            NavigationTarget::Document(DocumentId::from("chapter.md"))
        );
        assert_eq!(
            NavigationTarget::from_destination("chapter.md#results"),
            NavigationTarget::DocumentAnchor {
                document: DocumentId::from("chapter.md"),
                anchor: "results".into(),
            }
        );
        assert_eq!(
            NavigationTarget::from_destination("images/chart.png"),
            NavigationTarget::Asset(DocumentId::from("images/chart.png"))
        );
        assert_eq!(
            NavigationTarget::from_destination("https://example.com"),
            NavigationTarget::External("https://example.com".into())
        );
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReaderHistory {
    entries: Vec<DocumentLocation>,
    cursor: usize,
}

impl ReaderHistory {
    pub fn new(initial: DocumentLocation) -> Self {
        Self {
            entries: vec![initial],
            cursor: 0,
        }
    }

    pub fn current(&self) -> Option<&DocumentLocation> {
        self.entries.get(self.cursor)
    }

    pub fn can_go_back(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.cursor + 1 < self.entries.len()
    }

    pub fn push(&mut self, location: DocumentLocation) {
        if self.current() == Some(&location) {
            return;
        }
        self.entries.truncate(self.cursor + 1);
        self.entries.push(location);
        self.cursor = self.entries.len() - 1;
    }

    pub fn back(&mut self) -> Option<&DocumentLocation> {
        if !self.can_go_back() {
            return None;
        }
        self.cursor -= 1;
        self.current()
    }

    pub fn forward(&mut self) -> Option<&DocumentLocation> {
        if !self.can_go_forward() {
            return None;
        }
        self.cursor += 1;
        self.current()
    }

    pub fn entries(&self) -> &[DocumentLocation] {
        &self.entries
    }
}
