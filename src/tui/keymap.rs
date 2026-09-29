//! Keys to actions, from data/keybinds.toml.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::io::term::{KeyCode, KeyEvent, KeyModifiers};

const SRC: &str = include_str!("../../data/keybinds.toml");

static KEYMAP: LazyLock<Keymap> =
    LazyLock::new(|| Keymap::parse(SRC).expect("data/keybinds.toml is checked by tests"));

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Submit,
    Newline,
    Tab,
    CycleMode,
    Escape,
    CtrlC,
    Quit,
    ToggleReasoning,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    Backspace,
    Delete,
    DeleteWord,
    KillToStart,
    KillToEnd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Chord {
    code: KeyCode,
    ctrl: bool,
    alt: bool,
    shift: bool,
}

pub struct Keymap {
    bindings: Vec<(Chord, Action)>,
}

pub fn get() -> &'static Keymap {
    &KEYMAP
}

impl Keymap {
    fn parse(src: &str) -> Result<Self, String> {
        let table: BTreeMap<Action, Vec<String>> =
            toml::from_str(src).map_err(|e| e.to_string())?;
        let mut bindings = Vec::new();
        for (action, keys) in table {
            for key in keys {
                bindings.push((chord(&key)?, action));
            }
        }
        Ok(Self { bindings })
    }

    pub fn action(&self, key: &KeyEvent) -> Option<Action> {
        let pressed = normalize(key);
        self.bindings
            .iter()
            .find(|(chord, _)| *chord == pressed)
            .map(|(_, action)| *action)
    }
}

/// What was pressed, in the form bindings are written: letters lower-case, Shift folded into
/// the character it produced, Shift+Tab as BackTab.
fn normalize(key: &KeyEvent) -> Chord {
    let m = key.modifiers;
    let (code, shift) = match key.code {
        KeyCode::Char(c) => (KeyCode::Char(c.to_ascii_lowercase()), false),
        KeyCode::BackTab => (KeyCode::BackTab, false),
        code => (code, m.contains(KeyModifiers::SHIFT)),
    };
    Chord {
        code,
        ctrl: m.contains(KeyModifiers::CONTROL),
        alt: m.contains(KeyModifiers::ALT),
        shift,
    }
}

fn chord(spec: &str) -> Result<Chord, String> {
    let mut chord = Chord {
        code: KeyCode::Null,
        ctrl: false,
        alt: false,
        shift: false,
    };
    let parts: Vec<&str> = spec.split('+').collect();
    let (key, mods) = parts
        .split_last()
        .ok_or_else(|| format!("empty key `{spec}`"))?;
    for m in mods {
        match *m {
            "ctrl" => chord.ctrl = true,
            "alt" => chord.alt = true,
            "shift" => chord.shift = true,
            other => return Err(format!("unknown modifier `{other}` in `{spec}`")),
        }
    }
    chord.code = match *key {
        "enter" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "esc" => KeyCode::Esc,
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        k if k.chars().count() == 1 => KeyCode::Char(k.chars().next().unwrap_or(' ')),
        other => return Err(format!("unknown key `{other}` in `{spec}`")),
    };
    // Shift+Tab arrives as BackTab, with or without Shift reported.
    if chord.code == KeyCode::Tab && chord.shift {
        chord.code = KeyCode::BackTab;
        chord.shift = false;
    }
    if matches!(chord.code, KeyCode::Char(_)) {
        chord.shift = false;
    }
    Ok(chord)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::term::KeyEventKind;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new_with_kind(code, modifiers, KeyEventKind::Press)
    }

    #[test]
    fn data_parses_and_maps_what_windows_terminal_sends() {
        let map = get();
        // As reported by `crowbot keytest` on Windows Terminal.
        assert_eq!(
            map.action(&key(KeyCode::BackTab, KeyModifiers::SHIFT)),
            Some(Action::CycleMode)
        );
        assert_eq!(
            map.action(&key(KeyCode::Enter, KeyModifiers::SHIFT)),
            Some(Action::Newline)
        );
        assert_eq!(
            map.action(&key(KeyCode::Char('j'), KeyModifiers::CONTROL)),
            Some(Action::Newline)
        );
        assert_eq!(
            map.action(&key(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Action::Submit)
        );
        assert_eq!(
            map.action(&key(
                KeyCode::Char('C'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )),
            Some(Action::CtrlC)
        );
        assert_eq!(
            map.action(&key(KeyCode::Char('x'), KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn bad_specs_are_rejected() {
        assert!(chord("hyper+x").is_err());
        assert!(chord("ctrl+nosuchkey").is_err());
    }
}
