//! This project's sessions on disk.

use std::path::PathBuf;

use jiff::Timestamp;

use crate::config::limits;
use crate::config::paths::Paths;
use crate::conversation::message::{Message, Part};
use crate::io;
use crate::session::reader;

/// One session as a list of them shows it.
pub struct Summary {
    pub path: PathBuf,
    pub modified: Option<Timestamp>,
    /// The first prompt of its current branch.
    pub first_prompt: String,
    pub prompts: usize,
}

/// This project's sessions that say something, newest first, at most `session.resume_list`.
// CEILING: reads every file whole to summarise it. Upgrade: an index beside the files, kept
// by the store, once a project has sessions enough to make /resume slow.
pub fn list(paths: &Paths) -> Vec<Summary> {
    let mut files = files(paths);
    files.sort_by_key(|path| std::cmp::Reverse(io::fs::modified(path)));
    files
        .into_iter()
        .filter_map(|path| {
            let text = io::fs::read_string(&path).ok()??;
            let prompts: Vec<String> = reader::messages(&text)
                .iter()
                .filter_map(|m| match m {
                    Message::User { parts } => Some(Part::text_of(parts)),
                    _ => None,
                })
                .collect();
            let first_prompt = prompts.first()?.clone();
            Some(Summary {
                modified: io::fs::modified(&path).and_then(|t| Timestamp::try_from(t).ok()),
                path,
                first_prompt,
                prompts: prompts.len(),
            })
        })
        .take(limits::get().session.resume_list.value)
        .collect()
}

fn files(paths: &Paths) -> Vec<PathBuf> {
    let dir = paths.sessions_dir();
    io::fs::list_dir(&dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| !e.is_dir && e.name.ends_with(".jsonl"))
        .map(|e| dir.join(e.name))
        .collect()
}

/// The session file written to last, if this project has one.
pub fn latest(paths: &Paths) -> Option<PathBuf> {
    files(paths)
        .into_iter()
        .max_by_key(|path| io::fs::modified(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::transcript::Transcript;

    #[test]
    fn the_latest_is_the_one_written_to_last() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().into());
        assert_eq!(latest(&paths), None);
        let mut older = Transcript::create(&paths);
        older.push(Message::user_text("one")).unwrap();
        let mut newer = Transcript::create(&paths);
        newer.push(Message::user_text("two")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        older.push(Message::user_text("three")).unwrap();
        assert_eq!(latest(&paths).as_deref(), older.path());

        let listed = list(&paths);
        assert_eq!(listed.len(), 2);
        assert_eq!(Some(listed[0].path.as_path()), older.path());
        assert_eq!(
            (listed[0].first_prompt.as_str(), listed[0].prompts),
            ("one", 2)
        );
        assert_eq!(listed[1].first_prompt, "two");
    }
}
