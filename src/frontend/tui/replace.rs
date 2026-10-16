//! The one question before a new key replaces the key stored on this machine, worded by what
//! replacing it would lose.

use crate::auth::{self, KeyKind};
use crate::config::paths::Paths;
use crate::frontend::tui::boxed;
use crate::frontend::tui::choice::{Choice, Opt};
use crate::frontend::tui::ui;
use crate::text::styled::{Line, Style};
use crate::text::template::fill;

/// Which stored keys are worth asking about.
pub enum Asks {
    /// Only what cannot be got back: an account number, or a file nothing can read. A device
    /// key pairs again.
    Irreplaceable,
    Any,
}

/// The question, or `None` when nothing stored is worth asking about. Answers
/// `Reply::Choice(0)` to replace it; `width` wraps its words.
pub fn question(paths: &Paths, asks: Asks, width: usize) -> Option<Choice> {
    let text = &ui::get().replace;
    let said = match auth::held(paths) {
        Ok(None) => return None,
        Ok(Some((KeyKind::Device, hint))) => match asks {
            Asks::Irreplaceable => return None,
            Asks::Any => fill(&text.device, &[("hint", &hint)]),
        },
        Ok(Some((KeyKind::Account, hint))) => fill(&text.account, &[("hint", &hint)]),
        Err(e) => fill(&text.unreadable, &[("error", &e.to_string())]),
    };
    let body = Line::styled(said, Style::fg("warn")).wrap(boxed::inner(width), &Line::default());
    let options = vec![
        Opt::index(text.replace.clone(), 0),
        Opt::index(text.keep.clone(), 1),
    ];
    Some(Choice::new(text.title.clone(), body, options))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::card::Card;

    fn shown(choice: &Choice) -> String {
        choice
            .render(60)
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn it_asks_about_an_account_number_or_an_unreadable_file_and_a_device_key_only_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().join("home"), dir.path().to_path_buf());
        assert!(question(&paths, Asks::Any, 60).is_none());

        auth::save(&paths, "device-key-9999", KeyKind::Device).unwrap();
        assert!(question(&paths, Asks::Irreplaceable, 60).is_none());
        let asked = shown(&question(&paths, Asks::Any, 60).unwrap());
        assert!(asked.contains("device key …9999"), "{asked}");

        auth::save(&paths, "1234 5678 9012 3456", KeyKind::Account).unwrap();
        let asked = shown(&question(&paths, Asks::Irreplaceable, 60).unwrap());
        assert!(asked.contains("account number …3456"), "{asked}");
        assert!(
            asked.contains("1 Replace it") && asked.contains("2 Keep it"),
            "{asked}"
        );

        crate::io::fs::write_atomic(&paths.auth(), b"not json", crate::io::fs::Access::Private)
            .unwrap();
        let asked = shown(&question(&paths, Asks::Irreplaceable, 60).unwrap());
        assert!(asked.contains("unreadable"), "{asked}");
    }
}
