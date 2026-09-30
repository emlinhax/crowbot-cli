use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

use super::edit_match::{self, Miss};
use super::permissions;
use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail, target};
use crate::io::{self, fs::Access, fs::Kind};

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "edit",
        include_str!("../../data/tools/edit.md"),
        include_str!("../../data/tools/edit.schema.json"),
    )
});

#[derive(Deserialize)]
struct Args {
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

impl Args {
    fn edits(self) -> Result<Vec<EditSpec>, Refusal> {
        match (self.edits, self.old_text, self.new_text) {
            (Some(Edits::List(list)), ..) => Ok(list),
            (Some(Edits::One(one)), ..) => Ok(vec![one]),
            (Some(Edits::Text(text)), ..) => serde_json::from_str::<Vec<EditSpec>>(&text)
                .or_else(|_| serde_json::from_str::<EditSpec>(&text).map(|e| vec![e]))
                .map_err(|e| Refusal::InvalidArgs(format!("edits: {e}"))),
            (None, Some(old_text), Some(new_text)) => Ok(vec![EditSpec {
                old_text,
                new_text,
                replace_all: false,
            }]),
            _ => Err(Refusal::InvalidArgs("`edits` is required".into())),
        }
    }
}

pub struct Edit;

impl Tool for Edit {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args: Args = parse(args)?;
        let target =
            target::resolve(&cx.app.paths.project, &args.path).map_err(Refusal::Refused)?;
        let edits = args.edits()?;
        if edits.is_empty() {
            return Err(Refusal::InvalidArgs("`edits` is empty".into()));
        }
        if io::fs::kind(&target.path) != Kind::File {
            return Err(Refusal::Refused(format!(
                "{} is not a file; use write to create it.",
                target.shown
            )));
        }
        // A call that will fail is refused now, before anyone is asked to approve it.
        cx.files()
            .check_fresh(&target.path, &target.shown)
            .map_err(Refusal::Refused)?;
        let before = io::fs::read_string(&target.path)
            .ok()
            .flatten()
            .unwrap_or_default();
        let applied = apply(&before, &edits)
            .map_err(|why| Refusal::Refused(format!("{why} ({})", target.shown)))?;
        Ok(
            Check::new(target::asks(permissions::EDIT.name, &target)).with_preview(diff(
                &before,
                &applied.text,
                &target.shown,
            )),
        )
    }

    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let args: Args = match parse_or_fail(&args) {
                Ok(args) => args,
                Err(out) => return out,
            };
            let target = match target::resolve(&cx.app.paths.project, &args.path) {
                Ok(target) => target,
                Err(why) => return Output::error(why),
            };
            let edits = match args.edits() {
                Ok(edits) => edits,
                Err(e) => return Output::error(e.to_string()),
            };
            let _guard = cx.files().lock(&target.path).await;
            if let Err(why) = cx.files().check_fresh(&target.path, &target.shown) {
                return Output::error(why);
            }
            let before = match io::fs::read_string(&target.path) {
                Ok(Some(text)) => text,
                Ok(None) => return Output::error(format!("{} does not exist.", target.shown)),
                Err(e) => return Output::error(format!("Could not read {}: {e}", target.shown)),
            };
            let applied = match apply(&before, &edits) {
                Ok(applied) => applied,
                Err(why) => return Output::error(format!("{} ({})", why, target.shown)),
            };
            if let Err(e) =
                io::fs::write_atomic(&target.path, applied.text.as_bytes(), Access::Shared)
            {
                return Output::error(format!("Could not write {}: {e}", target.shown));
            }
            cx.files().saw(&target.path);
            let diff = diff(&before, &applied.text, &target.shown);
            Output::ok(format!(
                "Edited {}: {} replacement{}.",
                target.shown,
                applied.replacements,
                if applied.replacements == 1 { "" } else { "s" }
            ))
            .with_details(json!({"diff": diff}))
        })
    }
}

fn diff(before: &str, after: &str, shown: &str) -> String {
    similar::TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header(shown, shown)
        .to_string()
}

pub struct Applied {
    pub text: String,
    pub replacements: usize,
}

/// Applies every edit to `original`, all matched against the original text.
pub fn apply(original: &str, edits: &[EditSpec]) -> Result<Applied, String> {
    let bom = original.starts_with('\u{feff}');
    let body = original.strip_prefix('\u{feff}').unwrap_or(original);
    let crlf = body.contains("\r\n");
    let content = body.replace("\r\n", "\n");

    let mut replacements = Vec::new();
    for (i, edit) in edits.iter().enumerate() {
        let label = if edits.len() == 1 {
            String::from("old_text")
        } else {
            format!("edits[{i}].old_text")
        };
        let old = edit.old_text.replace("\r\n", "\n");
        let new = edit.new_text.replace("\r\n", "\n");
        if old.is_empty() {
            return Err(format!("{label} is empty; use write to create a file"));
        }
        if old == new {
            return Err(format!("{label} and new_text are identical"));
        }
        let found = edit_match::find(&content, &old, edit.replace_all).map_err(|miss| match miss {
            Miss::NotFound { hint } => {
                let mut why = format!("{label} was not found");
                if let Some(hint) = hint {
                    why.push_str(". ");
                    why.push_str(&hint);
                }
                why.push_str(". Read the file again and copy the text exactly");
                why
            }
            Miss::Ambiguous(n) => format!(
                "{label} matches {n} places; include more surrounding lines to make it unique, or set replace_all"
            ),
        })?;
        for span in found.spans {
            let new = if found.by_lines {
                reindent(
                    &new,
                    edit_match::indent_of(&old),
                    edit_match::indent_of(&content[span.start..span.end]),
                )
            } else {
                new.clone()
            };
            replacements.push((span, i, new));
        }
    }

    replacements.sort_by_key(|(span, ..)| span.start);
    for pair in replacements.windows(2) {
        if pair[0].0.end > pair[1].0.start {
            return Err(format!(
                "edits[{}] and edits[{}] overlap; merge them into one edit",
                pair[0].1, pair[1].1
            ));
        }
    }
    let count = replacements.len();
    let mut text = content.clone();
    for (span, _, new) in replacements.iter().rev() {
        text.replace_range(span.start..span.end, new);
    }
    if text == content {
        return Err("the edits change nothing".into());
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
    })
}

/// When a block matched at a deeper indent than the model wrote, shift the replacement to match.
fn reindent(new: &str, written: &str, actual: &str) -> String {
    let Some(extra) = actual.strip_prefix(written).filter(|e| !e.is_empty()) else {
        return new.to_owned();
    };
    new.split('\n')
        .map(|line| {
            if line.trim().is_empty() {
                line.to_owned()
            } else {
                format!("{extra}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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
        let update = crate::settings::env("UPDATE_GOLDEN").is_some();
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
        crate::tools::read::Read
            .run(json!({"path": "a.rs"}), &project.cx())
            .await;
        let out = Edit
            .run(
                json!({"path": "a.rs", "edits": [{"old_text": "    1", "new_text": "    2"}]}),
                &project.cx(),
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

    #[tokio::test]
    async fn refuses_files_it_has_not_read() {
        let project = Project::new();
        project.write("a.rs", "x");
        let out = Edit
            .run(
                json!({"path": "a.rs", "old_text": "x", "new_text": "y"}),
                &project.cx(),
            )
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("Read a.rs"), "{}", out.content);
    }
}
