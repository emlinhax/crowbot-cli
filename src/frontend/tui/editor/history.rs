//! Entries the session's prompt can step back through, with the draft set aside meanwhile.

#[derive(Default)]
pub struct History {
    entries: Vec<String>,
    /// Which entry is shown while browsing; `None` while editing a fresh draft.
    browsing: Option<usize>,
    /// The draft set aside while browsing.
    draft: String,
    max: usize,
}

impl History {
    pub fn new(max: usize) -> Self {
        Self {
            max,
            ..Self::default()
        }
    }

    /// Keeps `entry` unless it is blank or repeats the last one; past `max` the oldest goes.
    pub fn remember(&mut self, entry: String) {
        if entry.trim().is_empty() || self.entries.last() == Some(&entry) {
            return;
        }
        self.entries.push(entry);
        if self.entries.len() > self.max {
            self.entries.remove(0);
        }
    }

    /// Stops browsing; the next step back starts from the newest entry.
    pub fn reset(&mut self) {
        self.browsing = None;
    }

    /// The entry before the one shown; the first step sets `current` aside as the draft.
    pub fn back(&mut self, current: String) -> Option<String> {
        let next = match self.browsing {
            None if self.entries.is_empty() => return None,
            None => {
                self.draft = current;
                self.entries.len() - 1
            }
            Some(0) => return None,
            Some(i) => i - 1,
        };
        self.browsing = Some(next);
        Some(self.entries[next].clone())
    }

    /// The entry after the one shown, or the draft once past the newest.
    pub fn forward(&mut self) -> Option<String> {
        let i = self.browsing?;
        if i + 1 < self.entries.len() {
            self.browsing = Some(i + 1);
            Some(self.entries[i + 1].clone())
        } else {
            self.browsing = None;
            Some(std::mem::take(&mut self.draft))
        }
    }
}
