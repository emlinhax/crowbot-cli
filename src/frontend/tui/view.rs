//! Where the transcript is scrolled to. It follows the newest output until scrolled up, holds
//! still while more arrives, and follows again once scrolled back to the bottom.

#[derive(Default)]
pub struct View {
    /// The first row shown, while not following.
    top: Option<usize>,
    /// The transcript and viewport heights of the last frame, which scrolling moves within.
    total: usize,
    height: usize,
}

impl View {
    /// The first row to show of `total` in `height` rows.
    pub fn top(&mut self, total: usize, height: usize) -> usize {
        self.total = total;
        self.height = height;
        let bottom = self.bottom();
        match self.top {
            Some(top) if top < bottom => top,
            _ => {
                self.top = None;
                bottom
            }
        }
    }

    /// Moves by `rows`, negative is up; reaching the bottom follows again.
    pub fn scroll(&mut self, rows: isize) {
        let bottom = self.bottom();
        let to = self
            .top
            .unwrap_or(bottom)
            .saturating_add_signed(rows)
            .min(bottom);
        self.top = (to < bottom).then_some(to);
    }

    /// A page is the viewport less one row, so a line of context stays in view.
    pub fn page(&self) -> isize {
        self.height.saturating_sub(1).max(1) as isize
    }

    fn bottom(&self) -> usize {
        self.total.saturating_sub(self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_until_scrolled_up_then_holds_still() {
        let mut view = View::default();
        assert_eq!(view.top(5, 10), 0, "short transcripts start at the top");
        assert_eq!(view.top(30, 10), 20);
        view.scroll(-3);
        assert_eq!(view.top(30, 10), 17);
        // New output arrives; the view stays put.
        assert_eq!(view.top(40, 10), 17);
        view.scroll(-100);
        assert_eq!(view.top(40, 10), 0);
    }

    #[test]
    fn scrolling_back_to_the_bottom_follows_again() {
        let mut view = View::default();
        view.top(30, 10);
        view.scroll(-5);
        view.scroll(100);
        assert_eq!(view.top(50, 10), 40);
        assert_eq!(view.page(), 9);
    }
}
