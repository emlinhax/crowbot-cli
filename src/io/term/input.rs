//! Keys, pastes, resizes and the mouse, as a stream of what the UI reacts to.

use crossterm::event::{Event, EventStream, MouseButton, MouseEventKind};
// CEILING: key types are crossterm's; a backend swap needs a mirror type.
pub use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::{Stream, StreamExt};

/// What the UI reacts to.
#[derive(Debug, PartialEq, Eq)]
pub enum Input {
    Key(KeyEvent),
    Paste(String),
    Resize(usize, usize),
    /// Wheel notches: negative is up.
    Scroll(isize),
    /// A press on a screen cell, zero-based.
    Click {
        row: usize,
        column: usize,
        button: Button,
    },
    /// The pointer moved to a cell with the left button held.
    Drag {
        row: usize,
        column: usize,
    },
    /// The left button let go over a cell.
    Release {
        row: usize,
        column: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

/// Terminal input as a stream.
pub fn inputs() -> impl Stream<Item = Input> {
    EventStream::new().filter_map(|event| async move { input(event.ok()?) })
}

/// Key releases are dropped (Windows reports one for every key, which would otherwise act
/// twice), and so are bare mouse moves, the right button's release and the middle button.
fn input(event: Event) -> Option<Input> {
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => Some(Input::Key(key)),
        Event::Paste(text) => Some(Input::Paste(text)),
        Event::Resize(w, h) => Some(Input::Resize(w as usize, h as usize)),
        Event::Mouse(mouse) => {
            let (row, column) = (mouse.row as usize, mouse.column as usize);
            let button = match mouse.kind {
                MouseEventKind::ScrollUp => return Some(Input::Scroll(-1)),
                MouseEventKind::ScrollDown => return Some(Input::Scroll(1)),
                MouseEventKind::Drag(MouseButton::Left) => {
                    return Some(Input::Drag { row, column });
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    return Some(Input::Release { row, column });
                }
                MouseEventKind::Down(MouseButton::Left) => Button::Left,
                MouseEventKind::Down(MouseButton::Right) => Button::Right,
                _ => return None,
            };
            Some(Input::Click {
                row,
                column,
                button,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::MouseEvent;

    fn mouse(kind: MouseEventKind, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column: 3,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn both_buttons_the_wheel_and_a_left_drag_arrive_and_the_rest_of_the_mouse_does_not() {
        let click = |button| {
            Some(Input::Click {
                row: 4,
                column: 3,
                button,
            })
        };
        assert_eq!(
            input(mouse(MouseEventKind::Down(MouseButton::Left), 4)),
            click(Button::Left)
        );
        assert_eq!(
            input(mouse(MouseEventKind::Down(MouseButton::Right), 4)),
            click(Button::Right)
        );
        assert_eq!(
            input(mouse(MouseEventKind::ScrollUp, 0)),
            Some(Input::Scroll(-1))
        );
        assert_eq!(
            input(mouse(MouseEventKind::Drag(MouseButton::Left), 4)),
            Some(Input::Drag { row: 4, column: 3 })
        );
        assert_eq!(
            input(mouse(MouseEventKind::Up(MouseButton::Left), 4)),
            Some(Input::Release { row: 4, column: 3 })
        );
        for ignored in [
            MouseEventKind::Down(MouseButton::Middle),
            MouseEventKind::Drag(MouseButton::Right),
            MouseEventKind::Up(MouseButton::Right),
            MouseEventKind::Moved,
        ] {
            assert_eq!(input(mouse(ignored, 4)), None, "{ignored:?}");
        }
    }
}
