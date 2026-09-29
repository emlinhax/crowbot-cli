//! Code colouring with syntect's bundled grammars, mapped to theme roles by scope selectors
//! from data/theme.toml, so code follows the same palette as everything else.
//! CEILING: syntect's default grammar set (no TypeScript or TOML of its own; see
//! data/code_aliases.toml). The upgrade is a curated dump built with `dump-create`.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::LazyLock;

use serde::Deserialize;
use syntect::highlighting::ScopeSelectors;
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::text::styled::{Line, Style};
use crate::text::theme;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

static RULES: LazyLock<Vec<(ScopeSelectors, String)>> = LazyLock::new(|| {
    theme::get()
        .syntax
        .iter()
        .map(|rule| {
            let selectors = ScopeSelectors::from_str(&rule.scope)
                .unwrap_or_else(|e| panic!("bad scope `{}` in data/theme.toml: {e}", rule.scope));
            (selectors, rule.role.clone())
        })
        .collect()
});

static ALIASES: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        alias: BTreeMap<String, String>,
    }
    toml::from_str::<File>(include_str!("../../data/code_aliases.toml"))
        .expect("data/code_aliases.toml is checked by tests")
        .alias
});

/// `code` as lines coloured for `lang`; an unknown language comes back uncoloured.
pub fn highlight(code: &str, lang: &str) -> Vec<Line> {
    let Some(syntax) = find(lang) else {
        return code.lines().map(Line::plain).collect();
    };
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out = Vec::new();
    for raw in LinesWithEndings::from(code) {
        let mut line = Line::default();
        let Ok(ops) = state.parse_line(raw, &SYNTAXES) else {
            out.push(Line::plain(raw.trim_end_matches(['\n', '\r'])));
            continue;
        };
        let mut at = 0;
        for (pos, op) in ops {
            if pos > at {
                push(&mut line, &raw[at..pos], &stack);
            }
            at = pos;
            let _ = stack.apply(&op);
        }
        if at < raw.len() {
            push(&mut line, &raw[at..], &stack);
        }
        out.push(line);
    }
    out
}

fn find(lang: &str) -> Option<&'static SyntaxReference> {
    let lang = lang.trim().to_lowercase();
    if lang.is_empty() {
        return None;
    }
    let lang = ALIASES.get(&lang).map_or(lang.as_str(), String::as_str);
    SYNTAXES.find_syntax_by_token(lang)
}

fn push(line: &mut Line, text: &str, stack: &ScopeStack) {
    let text = text.trim_end_matches(['\n', '\r']);
    if text.is_empty() {
        return;
    }
    let role = RULES
        .iter()
        .filter_map(|(selectors, role)| selectors.does_match(stack.as_slice()).map(|p| (p, role)))
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, role)| role.as_str());
    line.push(text, role.map(Style::fg).unwrap_or_default());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_known_languages_by_role() {
        let lines = highlight("fn main() {\n    // hi\n    let x = \"s\";\n}\n", "rust");
        let tagged: Vec<String> = lines.iter().map(Line::to_tagged).collect();
        assert!(tagged[0].contains("[syn_keyword]fn[/]"), "{tagged:?}");
        assert!(tagged[0].contains("[syn_function]main[/]"), "{tagged:?}");
        assert!(tagged[1].contains("[syn_comment]"), "{tagged:?}");
        assert!(tagged[2].contains("[syn_string]"), "{tagged:?}");
    }

    #[test]
    fn aliases_and_unknown_languages() {
        assert!(find("ts").is_some());
        assert!(find("SH").is_some());
        let plain = highlight("a\nb", "no-such-language");
        assert_eq!(
            plain.iter().map(Line::to_tagged).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn every_alias_points_at_a_bundled_grammar() {
        for (from, to) in ALIASES.iter() {
            assert!(
                SYNTAXES.find_syntax_by_token(to).is_some(),
                "{from} -> {to}"
            );
        }
    }
}
