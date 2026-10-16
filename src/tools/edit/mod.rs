//! The edit tool: replaces text the model has read, refusing a call that would fail before the
//! user is asked.

mod apply;
mod find;

use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use apply::{EditSpec, apply};

use super::target::{self, Target};
use super::write::save;
use super::{Check, Output, Refusal, Spec, Tool, ToolCx};
use crate::data;
use crate::io::{self, fs::Kind};
use crate::permission::catalog;
use crate::text::diff;
use crate::text::template::fill;
use crate::text::units::Plural;

static SPEC: LazyLock<Spec> = crate::tools::spec!("edit");

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::testing::Project;

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
