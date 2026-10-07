//! Roles to colours, from data/theme.toml. Rendering only ever names roles.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

const SRC: &str = include_str!("../../data/theme.toml");

static THEME: LazyLock<Theme> = LazyLock::new(|| Theme::parse(SRC));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

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
}
