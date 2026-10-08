//! One value or a list of them, so a tool takes a single target or a batch in one call, and how a
//! batch's results are joined into one reply.

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(untagged)]
pub enum Many<T> {
    One(T),
    Set(Vec<T>),
}

impl<T> Many<T> {
    /// The items, at most `max` of them.
    pub fn capped(&self, max: usize) -> &[T] {
        let items = match self {
            Self::One(item) => std::slice::from_ref(item),
            Self::Set(items) => items,
        };
        &items[..items.len().min(max)]
    }
}

/// Each result under its label, between rules; a single result stands alone, unlabelled.
pub fn join(parts: Vec<(String, String)>) -> String {
    let solo = parts.len() == 1;
    let mut out = String::new();
    for (label, text) in parts {
        if !out.is_empty() {
            out.push_str("\n\n───\n\n");
        }
        if !solo {
            out.push_str(&label);
            out.push('\n');
        }
        out.push_str(&text);
    }
    out
}
