//! The line above the editor while crowbot works: what it is doing and how to stop it.

use crate::text::styled::{Line, Style};
use crate::tui::ui;

pub enum Status<'a> {
    Working { secs: u64, frame: &'a str },
    Retrying { secs: u64, why: &'a str },
}

pub fn render(status: &Status<'_>, width: usize) -> Line {
    let text = &ui::get().text;
    let muted = Style::fg("muted");
    let line = match status {
        Status::Working { secs, frame } => {
            let mut line = Line::styled(format!("{frame} "), Style::fg("accent"));
            line.push(format!("{} {secs}s", text.working), Style::default());
            line.push(format!(" · {}", text.interrupt), muted);
            line
        }
        Status::Retrying { secs, why } => {
            let mut line = Line::styled(format!("{why} · "), Style::fg("warn"));
            line.push(format!("{} {secs}s", text.retrying), muted.clone());
            line.push(format!(" · {}", text.interrupt), muted);
            line
        }
    };
    line.truncate(width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_and_retrying() {
        let working = render(
            &Status::Working {
                secs: 12,
                frame: "⠋",
            },
            80,
        );
        assert_eq!(working.text(), "⠋ Working 12s · esc to interrupt");
        let retry = render(
            &Status::Retrying {
                secs: 3,
                why: "Too many requests at once",
            },
            80,
        );
        assert_eq!(
            retry.text(),
            "Too many requests at once · retrying in 3s · esc to interrupt"
        );
    }
}
