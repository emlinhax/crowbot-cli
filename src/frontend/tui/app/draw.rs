//! One frame: the conversation, the card or the editor, the bar and what floats over them.

use super::Tui;
use crate::config::limits;
use crate::frontend::tui::{bar, layout, queue, ui};
use crate::io::{clock, term};
use crate::text::styled::{Line, Style};
use std::time::Instant;

impl<'a> Tui<'a> {
    pub(super) fn draw(&mut self, running: bool) {
        let now = clock::instant();
        let (pad, width) = layout::margins(self.screen.width());
        let height = self.screen.height();
        let mode = self.shared.mode();
        // The bar, pinned to the bottom of the screen.
        let mut live = Vec::new();
        if running {
            live.push(self.status(now, width));
        }
        live.extend(queue::render(&self.queue, width));
        if let Some(at) = self.last_ctrl_c
            && now.duration_since(at) <= limits::get().tui.quit_window_ms.ms()
            && !running
        {
            live.push(Line::styled(
                &ui::get().text.ctrl_c_again,
                Style::fg("muted"),
            ));
        }
        // Prompts answered elsewhere (AUTO approved them, the run was interrupted) go away.
        let shared = self.shared;
        self.cards.retain(|(id, _)| shared.prompts.waiting(*id));
        // The command popup floats over the conversation instead of growing the bar.
        let mut floating = Vec::new();
        let card = match &self.panel {
            Some(card) => Some(card.render(width)),
            None => self.cards.front().map(|(_, card)| card.render(width)),
        };
        match card {
            Some(lines) => live.extend(lines),
            None => {
                live.push(bar::top(mode, width));
                if !running {
                    let found = self.palette.open(&self.editor.text());
                    if !found.is_empty() {
                        floating = self.palette.render(&found, width);
                    }
                }
                let ui = ui::get();
                let prompt = Line::styled(&ui.prompt, Style::fg(&mode.color).bold());
                let rows = (height * limits::get().tui.editor_max_rows_pct.value / 100)
                    .max(limits::get().tui.editor_min_rows.value);
                live.extend(self.editor.render(width, rows, &prompt, &ui.placeholder));
            }
        }
        live.push(bar::bottom(
            mode,
            &bar::Info {
                model: &self.next.model.id,
                effort: self.next.effort(),
                context_pct: self.context_pct(),
                cost_micros: self.cost_micros,
                account: self.account.me.as_ref(),
            },
            width,
        ));
        let mut frame = layout::compose(
            &mut self.feed,
            &mut self.view,
            &live,
            &floating,
            (width, height),
            now,
        );
        self.blocks = frame.blocks;
        if self.toast.as_ref().is_some_and(|t| !t.shown(now)) {
            self.toast = None;
        }
        if let Some(toast) = &self.toast {
            toast.overlay(&mut frame.rows, width);
        }
        layout::inset(&mut frame.rows, pad);
        let bytes = self.screen.frame(&frame.rows);
        if !bytes.is_empty() {
            term::out(&bytes);
        }
    }

    /// The columns a frame is drawn in, inside the side margins.
    pub(super) fn draw_width(&self) -> usize {
        layout::margins(self.screen.width()).1
    }

    pub(super) fn status(&self, now: Instant, width: usize) -> Line {
        let color = &self.shared.mode().color;
        self.progress
            .as_ref()
            .map(|p| p.render(now, color, self.braille, width))
            .unwrap_or_default()
    }
}
