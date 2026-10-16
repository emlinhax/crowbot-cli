//! Applying a batch of edits to a file's text: each found in the original, then all replaced at
//! once, keeping the file's line endings and BOM.

use serde::Deserialize;

use super::TEXT;
use super::find::{self, Fit, Miss};
use crate::text::template::fill;

#[derive(Clone, Debug, Deserialize)]
pub struct EditSpec {
    pub old_text: String,
    pub new_text: String,
    #[serde(default)]
    pub replace_all: bool,
}

pub struct Applied {
    pub text: String,
    pub replacements: usize,
    /// Each replacement found by a loose strategy, `whitespace normalized, line 12`: the diff
    /// only reaches the user, so the model hears this and knows to check its edit.
    pub loose: Vec<String>,
}

/// Applies every edit to `original`, all matched against the original text.
pub fn apply(original: &str, edits: &[EditSpec]) -> Result<Applied, String> {
    let bom = original.starts_with('\u{feff}');
    let body = original.strip_prefix('\u{feff}').unwrap_or(original);
    let crlf = body.contains("\r\n");
    let content = body.replace("\r\n", "\n");

    let mut replacements = Vec::new();
    let mut loose = Vec::new();
    for (i, edit) in edits.iter().enumerate() {
        let label = if edits.len() == 1 {
            String::from("old_text")
        } else {
            format!("edits[{i}].old_text")
        };
        let old = edit.old_text.replace("\r\n", "\n");
        let new = edit.new_text.replace("\r\n", "\n");
        let named = [("label", label.as_str())];
        if old.is_empty() {
            return Err(fill(&TEXT.empty_old, &named));
        }
        if old == new {
            return Err(fill(&TEXT.identical, &named));
        }
        let found = find::find(&content, &old, edit.replace_all).map_err(|miss| match miss {
            Miss::NotFound { closest } => {
                let mut parts = vec![fill(&TEXT.not_found, &named)];
                parts.extend(closest.map(|c| {
                    fill(
                        &TEXT.closest,
                        &[
                            ("line", &c.line.to_string()),
                            ("percent", &format!("{:.0}", c.percent)),
                            ("text", &c.text),
                        ],
                    )
                }));
                parts.push(TEXT.reread.clone());
                parts.join(". ")
            }
            Miss::Ambiguous(n) => fill(
                &TEXT.ambiguous,
                &[("label", &label), ("count", &n.to_string())],
            ),
        })?;
        let loosely = found.loose();
        for span in found.spans {
            if loosely {
                let line = content[..span.start].matches('\n').count() + 1;
                loose.push(format!("{}, line {line}", found.strategy));
            }
            let new = match found.fit {
                Fit::Span => new.clone(),
                Fit::Trimmed => trim_like(&new, &old),
                Fit::Lines => reindent(
                    &new,
                    find::indent_of(&old),
                    find::indent_of(&content[span.start..span.end]),
                ),
            };
            replacements.push((span, i, new));
        }
    }

    replacements.sort_by_key(|(span, ..)| span.start);
    for pair in replacements.windows(2) {
        if pair[0].0.end > pair[1].0.start {
            return Err(fill(
                &TEXT.overlap,
                &[
                    ("first", &pair[0].1.to_string()),
                    ("second", &pair[1].1.to_string()),
                ],
            ));
        }
    }
    let count = replacements.len();
    let mut text = content.clone();
    for (span, _, new) in replacements.iter().rev() {
        text.replace_range(span.start..span.end, new);
    }
    if text == content {
        return Err(TEXT.no_change.clone());
    }
    // CEILING: a file mixing CRLF and LF comes back all CRLF; per-line endings would need
    // tracking each line's terminator through the replacement.
    if crlf {
        text = text.replace('\n', "\r\n");
    }
    if bom {
        text.insert(0, '\u{feff}');
    }
    Ok(Applied {
        text,
        replacements: count,
        loose,
    })
}

/// A block that matched at another indent than the model wrote: the written indent becomes the
/// actual one on every line of the replacement, deeper or shallower.
fn reindent(new: &str, written: &str, actual: &str) -> String {
    if written == actual {
        return new.to_owned();
    }
    new.split('\n')
        .map(|line| match line.strip_prefix(written) {
            Some(rest) if !line.trim().is_empty() => format!("{actual}{rest}"),
            _ => line.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The needle matched without the whitespace around it, so the replacement drops the same.
fn trim_like(new: &str, old: &str) -> String {
    let lead = &old[..old.len() - old.trim_start().len()];
    let trail = &old[old.trim_end().len()..];
    let new = new.strip_prefix(lead).unwrap_or(new);
    new.strip_suffix(trail).unwrap_or(new).to_owned()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)]

    use super::*;

    /// tests/golden/edit/<case>/: before.txt + args.json, and after.txt or error.txt (a substring
    /// of the expected error). `UPDATE_GOLDEN=1` rewrites the expectation from the output.
    #[test]
    fn golden_cases() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/edit");
        let update = crate::io::env::var("UPDATE_GOLDEN").is_some();
        let mut cases: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        cases.sort();
        assert!(
            cases.len() >= 10,
            "golden cases missing from {}",
            root.display()
        );
        for case in cases {
            let name = case.file_name().unwrap().to_string_lossy().into_owned();
            let before = std::fs::read_to_string(case.join("before.txt")).unwrap();
            let edits: Vec<EditSpec> =
                serde_json::from_str(&std::fs::read_to_string(case.join("args.json")).unwrap())
                    .unwrap();
            let after = std::fs::read_to_string(case.join("after.txt")).ok();
            let error = std::fs::read_to_string(case.join("error.txt")).ok();
            let result = apply(&before, &edits);
            let holds = match (&result, &after, &error) {
                (Ok(applied), Some(after), _) => applied.text == *after,
                (Err(why), _, Some(error)) => why.contains(error.trim()),
                _ => false,
            };
            // Only a failing expectation is rewritten; one that holds is left as written.
            if update && !holds {
                match &result {
                    Ok(applied) => std::fs::write(case.join("after.txt"), &applied.text).unwrap(),
                    Err(why) => std::fs::write(case.join("error.txt"), why).unwrap(),
                }
                continue;
            }
            match result {
                Ok(applied) => assert_eq!(Some(applied.text), after, "{name}"),
                Err(why) => assert!(holds, "{name}: {why}"),
            }
        }
    }

    #[test]
    fn a_loose_match_is_named_with_its_line() {
        let edit = |old: &str, new: &str| EditSpec {
            old_text: old.into(),
            new_text: new.into(),
            replace_all: false,
        };
        let applied = apply(
            "a\nfn f() {\n    let  x = 1;\n}\n",
            &[edit("let x  = 1;", "let x = 2;")],
        )
        .unwrap();
        assert_eq!(applied.loose, vec!["whitespace normalized, line 3"]);
        let exact = apply("a\nb\n", &[edit("b", "c")]).unwrap();
        assert!(exact.loose.is_empty());
    }
}
