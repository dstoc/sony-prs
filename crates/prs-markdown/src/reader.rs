//! Reader navigation state, kept separate from physical input and display IO.

use crate::navigation::{DocumentLocation, NavigationTarget, ReaderHistory};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReaderAction {
    NextPage,
    PreviousPage,
    Back,
    Forward,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReaderSession {
    location: DocumentLocation,
    history: ReaderHistory,
    page: usize,
    page_count: usize,
}

impl ReaderSession {
    pub fn new(location: DocumentLocation, page_count: usize) -> Self {
        Self {
            history: ReaderHistory::new(location.clone()),
            location,
            page: 0,
            page_count: page_count.max(1),
        }
    }

    pub fn location(&self) -> &DocumentLocation {
        &self.location
    }

    pub fn page(&self) -> usize {
        self.page
    }

    pub fn page_count(&self) -> usize {
        self.page_count
    }

    pub fn set_page_count(&mut self, page_count: usize) {
        self.page_count = page_count.max(1);
        self.page = self.page.min(self.page_count - 1);
    }

    pub fn next_page(&mut self) -> bool {
        if self.page + 1 < self.page_count {
            self.page += 1;
            true
        } else {
            false
        }
    }

    pub fn previous_page(&mut self) -> bool {
        if self.page > 0 {
            self.page -= 1;
            true
        } else {
            false
        }
    }

    pub fn open(&mut self, target: NavigationTarget) -> bool {
        let location = match target {
            NavigationTarget::Location(location) => location,
            NavigationTarget::Anchor(anchor) => {
                DocumentLocation::new(self.location.document.clone(), Some(anchor))
            }
            NavigationTarget::External(_) => return false,
        };
        self.history.push(location.clone());
        self.location = location;
        self.page = 0;
        true
    }

    pub fn back(&mut self) -> bool {
        let Some(location) = self.history.back().cloned() else {
            return false;
        };
        if location == self.location {
            return false;
        }
        self.location = location;
        self.page = 0;
        true
    }

    pub fn forward(&mut self) -> bool {
        let Some(location) = self.history.forward().cloned() else {
            return false;
        };
        if location == self.location {
            return false;
        }
        self.location = location;
        self.page = 0;
        true
    }

    pub fn history(&self) -> &ReaderHistory {
        &self.history
    }
}
