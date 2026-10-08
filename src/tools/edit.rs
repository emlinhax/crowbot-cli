use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use super::edit_match::{self, Fit, Miss};
use super::target::{self, Target};
use super::write::save;
use super::{Check, Output, Refusal, Spec, Tool, ToolCx};
use crate::data;
use crate::io::{self, fs::Kind};
use crate::permission::catalog;
use crate::text::diff;
use crate::text::template::fill;
use crate::text::units::Plural;

static SPEC: LazyLock<Spec> = super::spec!("edit");

static TEXT: LazyLock<Text> = data::load!("tools/edit.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    bad_edits: String,
    no_edits: String,
    empty_edits: String,
    missing: String,
    not_utf8: String,
    empty_old: String,
    identical: String,
    not_found: String,
    closest: String,
    reread: String,
    ambiguous: String,
    overlap: String,
    no_change: String,
    loose: String,
    edited: Plural,
}

#[derive(Deserialize)]
#[serde(try_from = "Sent")]
pub struct Args {
    path: String,
    edits: Vec<EditSpec>,
}

/// The call as sent, in whichever spelling of the edits the model used.
#[derive(Deserialize)]
struct Sent {
    path: String,
    #[serde(default)]
    edits: Option<Edits>,
    // Models trained on single-edit tools send these at the top level; accept them.
    #[serde(default)]
    old_text: Option<String>,
    #[serde(default)]
    new_text: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Edits {
    List(Vec<EditSpec>),
    One(EditSpec),
    /// The list sent as a JSON string, which some models do.
    Text(String),
}

#[derive(Clone, Debug, Deserialize)]
pub struct EditSpec {
    pub old_text: String,
    pub new_text: String,
    #[serde(default)]
    pub replace_all: bool,
}

impl TryFrom<Sent> for Args {
    type Error = String;

    fn try_from(sent: Sent) -> Result<Self, String> {
        let edits = match (sent.edits, sent.old_text, sent.new_text) {
            (Some(Edits::List(list)), ..) => list,
            (Some(Edits::One(one)), ..) => vec![one],
            (Some(Edits::Text(text)), ..) => serde_json::from_str::<Vec<EditSpec>>(&text)
                .or_else(|_| serde_json::from_str::<EditSpec>(&text).map(|e| vec![e]))
                .map_err(|e| fill(&TEXT.bad_edits, &[("error", &e.to_string())]))?,
            (None, Some(old_text), Some(new_text)) => vec![EditSpec {
                old_text,
                new_text,
                replace_all: false,
            }],
            _ => return Err(TEXT.no_edits.clone()),
        };
        Ok(Self {
            path: sent.path,
            edits,
        })
    }
}

pub struct Edit;

impl Tool for Edit {
    type Args = Args;

    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Self::Args, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let target = target::resolve(&cx.app.paths, &args.path)?;
        if args.edits.is_empty() {
            return Err(Refusal::InvalidArgs(TEXT.empty_edits.clone()));
        }
        // A call that will fail is refused now, before anyone is asked to approve it.
        let before = load(&target)?;
        cx.files().check_fresh(&target.path, &target.shown)?;
        let applied =
            apply(&before, &args.edits).map_err(|why| format!("{why} ({})", target.shown))?;
        Ok(
            Check::new(target::asks(catalog::EDIT.name, &target)).with_preview(diff::unified(
                &before,
                &applied.text,
                &target.shown,
            )),
        )
    }

    fn run<'a>(
        &'a self,
        args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let target = target::resolve(&cx.app.paths, &args.path)?;
            let _guard = cx.files().claim(&target.path, &target.shown).await?;
            let before = load(&target)?;
            let applied =
                apply(&before, &args.edits).map_err(|why| format!("{why} ({})", target.shown))?;
            save(cx, &target, &applied.text)?;
            let diff = diff::unified(&before, &applied.text, &target.shown);
            let loose = if applied.loose.is_empty() {
                String::new()
            } else {
                fill(&TEXT.loose, &[("matches", &applied.loose.join("; "))])
            };
            Ok(Output::ok(fill(
                &TEXT.edited.of(applied.replacements),
                &[("path", &target.shown), ("loose", &loose)],
            ))
            .with_details(json!({"diff": diff})))
        })
    }
}

pub struct Applied {
    pub text: String,
    pub replacements: usize,
    /// Each replacement found by a loose strategy, `whitespace normalized, line 12`: the diff
    /// only reaches the user, so the model hears this and knows to check its edit.
    pub loose: Vec<String>,
}

/// The file's text, read the same way for the check and the run, or why edit cannot work on it.
fn load(target: &Target) -> Result<String, String> {
    let path = [("path", target.shown.as_str())];
    let missing = || fill(&TEXT.missing, &path);
    match target.kind()? {
        Kind::File => {}
        Kind::Missing => return Err(missing()),
        Kind::Dir => return Err(fill(&target::TEXT.is_dir, &path)),
    }
    match io::fs::read_string(&target.path) {
        Ok(Some(text)) => Ok(text),
        Ok(None) => Err(missing()),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Err(fill(&TEXT.not_utf8, &path)),
        Err(e) => Err(fill(
            &target::TEXT.read_failed,
            &[("path", &target.shown), ("error", &e.to_string())],
        )),
    }
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
        let found =
            edit_match::find(&content, &old, edit.replace_all).map_err(|miss| match miss {
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
        for span in found.spans {
            if found.strategy != "exact" {
                let line = content[..span.start].matches('\n').count() + 1;
                loose.push(format!("{}, line {line}", found.strategy));
            }
            let new = match found.fit {
                Fit::Span => new.clone(),
                Fit::Trimmed => trim_like(&new, &old),
                Fit::Lines => reindent(
                    &new,
                    edit_match::indent_of(&old),
                    edit_match::indent_of(&content[span.start..span.end]),
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
    use crate::tools::testing::Project;

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

    #[tokio::test]
    async fn edits_a_read_file_and_reports_a_diff() {
        let project = Project::new();
        project.write("a.rs", "fn a() {\n    1\n}\n");
        project
            .run(&crate::tools::read::Read, json!({"path": "a.rs"}))
            .await;
        let out = project
            .run(
                &Edit,
                json!({"path": "a.rs", "edits": [{"old_text": "    1", "new_text": "    2"}]}),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(project.read("a.rs"), "fn a() {\n    2\n}\n");
        assert!(
            out.details.unwrap()["diff"]
                .as_str()
                .unwrap()
                .contains("+    2")
        );
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

    #[test]
    fn says_why_a_file_cannot_be_edited() {
        let project = Project::new();
        crate::io::fs::write_atomic(
            &project.app.paths.project.join("latin1.txt"),
            b"caf\xe9\n",
            crate::io::fs::Access::Shared,
        )
        .unwrap();
        project.write("d/x.txt", "x");
        let why = |path: &str| {
            let args = json!({"path": path, "old_text": "a", "new_text": "b"});
            match project.check(&Edit, &args) {
                Err(Refusal::Refused(why)) => why,
                _ => panic!("{path} was not refused"),
            }
        };
        assert_eq!(
            why("latin1.txt"),
            "latin1.txt is not UTF-8 text; edit would corrupt it."
        );
        assert_eq!(why("d"), "d is a directory.");
        assert_eq!(
            why("gone.txt"),
            "gone.txt does not exist; use write to create it."
        );
    }

    #[tokio::test]
    async fn refuses_files_it_has_not_read() {
        let project = Project::new();
        project.write("a.rs", "x");
        let out = project
            .run(
                &Edit,
                json!({"path": "a.rs", "old_text": "x", "new_text": "y"}),
            )
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("Read a.rs"), "{}", out.content);
    }
}
