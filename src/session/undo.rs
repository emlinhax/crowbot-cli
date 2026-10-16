//! Taking back the last prompt: it and everything after it leave the current branch.

use crate::conversation::message::{Message, Part};
use crate::session::entry::Snap;
use crate::session::transcript::{Transcript, Unsaved};

/// What was taken back.
pub struct Undone {
    /// The prompt's words, for the editor.
    pub prompt: String,
    /// The files before and after it, when its run changed any.
    pub snapshot: Option<Snap>,
    /// The session file could not record it; the conversation is rewound in memory only.
    pub unsaved: Option<Unsaved>,
}

/// Rewinds `transcript` to just before its last prompt; `None` when it has none.
pub fn undo(transcript: &mut Transcript) -> Option<Undone> {
    let at = transcript
        .messages
        .iter()
        .rposition(|m| matches!(m, Message::User { .. }))?;
    let Message::User { parts } = &transcript.messages[at] else {
        return None;
    };
    let prompt = Part::text_of(parts);
    let snapshot = transcript.snapshot_at(at).cloned();
    let unsaved = transcript.rewind(at).err();
    Some(Undone {
        prompt,
        snapshot,
        unsaved,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::paths::Paths;
    use crate::conversation::message::{Assistant, Finish};

    #[test]
    fn the_last_prompt_and_what_followed_it_are_taken_back() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().into());
        let mut transcript = Transcript::create(&paths);
        assert!(undo(&mut transcript).is_none());
        let reply = || Message::Assistant(Assistant::stub(Finish::Done, Vec::new()));
        transcript.push(Message::user_text("one")).unwrap();
        transcript.push(reply()).unwrap();
        transcript.push(Message::user_text("two")).unwrap();
        transcript.push(reply()).unwrap();

        let undone = undo(&mut transcript).unwrap();
        assert_eq!(undone.prompt, "two");
        assert!(undone.unsaved.is_none());
        assert_eq!(transcript.messages.len(), 2);
        assert_eq!(undo(&mut transcript).unwrap().prompt, "one");
        assert!(transcript.messages.is_empty());
    }
}
