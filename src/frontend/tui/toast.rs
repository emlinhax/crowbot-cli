//! A short note at the top right of the screen that goes away by itself, such as "Copied".

use std::time::Instant;

use crate::limits;
use crate::text::styled::{Line, Style};

pub struct Toast {
    text: String,
    until: Instant,
}

impl Toast {
    pub fn new(text: &str, now: Instant) -> Self {
        Self {
            text: text.to_owned(),
            until: now + limits::get().tui.toast_ms.ms(),
        }
    }

    pub fn shown(&self, now: Instant) -> bool {
        now < self.until
    }

    /// Over the right end of the first row, which keeps its width.
    pub fn overlay(&self, rows: &mut [Line], width: usize) {
        let Some(first) = rows.first_mut() else {
            return;
        };
        let pill = Line::styled(
            format!(" {} ", self.text),
            Style::fg("toast_text").on("toast"),
        );
        let keep = width.saturating_sub(pill.width() + 1);
        let mut line = first.truncate(keep).padded(keep + 1, &Style::default());
        line.extend(pill);
        *first = line;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_sits_at_the_right_end_of_the_first_row_and_then_goes() {
        let now = crate::io::clock::instant();
        let toast = Toast::new("Copied", now);
        let mut rows = vec![
            Line::plain("a row that runs on and on"),
            Line::plain("second"),
        ];
        toast.overlay(&mut rows, 30);
        assert_eq!(rows[0].width(), 30);
        assert!(rows[0].text().ends_with(" Copied "), "{}", rows[0].text());
        assert_eq!(rows[1].text(), "second");
        assert!(toast.shown(now));
        assert!(!toast.shown(now + limits::get().tui.toast_ms.ms()));
    }
}
