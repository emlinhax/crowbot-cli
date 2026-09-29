//! Roles to colours, from data/theme.toml. Rendering only ever names roles.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

const SRC: &str = include_str!("../../data/theme.toml");

static THEME: LazyLock<Theme> = LazyLock::new(|| Theme::parse(SRC));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// How many colours the terminal can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    TrueColor,
    Ansi256,
    /// `NO_COLOR`: attributes only.
    None,
}

pub struct Theme {
    /// `None` is the terminal's default colour.
    roles: BTreeMap<String, Option<Rgb>>,
    pub syntax: Vec<SyntaxRule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyntaxRule {
    pub scope: String,
    pub role: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    palette: BTreeMap<String, String>,
    roles: BTreeMap<String, String>,
    syntax: Vec<SyntaxRule>,
}

pub fn get() -> &'static Theme {
    &THEME
}

impl Theme {
    fn parse(src: &str) -> Self {
        let file: File = toml::from_str(src).expect("data/theme.toml is checked by tests");
        let resolve = |value: &str| -> Option<Rgb> {
            if value == "default" {
                return None;
            }
            let hex = file.palette.get(value).map_or(value, String::as_str);
            Some(hex_to_rgb(hex).unwrap_or_else(|| {
                panic!("theme colour `{value}` is not a palette name or #rrggbb")
            }))
        };
        let roles = file
            .roles
            .iter()
            .map(|(role, value)| (role.clone(), resolve(value)))
            .collect();
        Self {
            roles,
            syntax: file.syntax,
        }
    }

    /// The colour of `role`; tests fail on an unknown role, a release build falls back to default.
    pub fn color(&self, role: &str) -> Option<Rgb> {
        match self.roles.get(role) {
            Some(color) => *color,
            None => {
                debug_assert!(false, "no role `{role}` in data/theme.toml");
                None
            }
        }
    }

    #[cfg(test)]
    pub fn has(&self, role: &str) -> bool {
        self.roles.contains_key(role)
    }
}

fn hex_to_rgb(hex: &str) -> Option<Rgb> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(Rgb(byte(0)?, byte(2)?, byte(4)?))
}

/// The nearest xterm-256 colour, for terminals without truecolour.
pub fn to_256(Rgb(r, g, b): Rgb) -> u8 {
    let level = |v: u8| -> u8 {
        match v {
            0..=47 => 0,
            48..=114 => 1,
            _ => (v - 35) / 40,
        }
    };
    let (lr, lg, lb) = (level(r), level(g), level(b));
    let cube = 16 + 36 * lr + 6 * lg + lb;
    let step = |l: u8| if l == 0 { 0 } else { 55 + 40 * l as u16 };
    let cube_err = [(r, lr), (g, lg), (b, lb)]
        .iter()
        .map(|(v, l)| (i32::from(*v) - i32::from(step(*l))).pow(2))
        .sum::<i32>();
    let avg = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    let gray_index = if avg > 238 {
        23
    } else {
        avg.saturating_sub(3) / 10
    } as u8;
    let gray = 8 + 10 * i32::from(gray_index);
    let gray_err = [r, g, b]
        .iter()
        .map(|v| (i32::from(*v) - gray).pow(2))
        .sum::<i32>();
    if gray_err < cube_err {
        232 + gray_index
    } else {
        cube
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_role_and_syntax_rule_resolves() {
        let theme = get();
        assert!(theme.has("accent"));
        assert_eq!(theme.color("accent"), Some(Rgb(0x00, 0xDC, 0x82)));
        assert_eq!(theme.color("text"), None);
        for rule in &theme.syntax {
            assert!(
                theme.has(&rule.role),
                "syntax rule names unknown role {}",
                rule.role
            );
        }
    }

    #[test]
    fn nearest_256_colours() {
        assert_eq!(to_256(Rgb(0, 0, 0)), 16);
        assert_eq!(to_256(Rgb(255, 255, 255)), 231);
        assert_eq!(to_256(Rgb(128, 128, 128)), 244);
        assert_eq!(to_256(Rgb(0, 220, 130)), 42);
    }
}
