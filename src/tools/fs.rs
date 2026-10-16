//! Moving, copying and deleting files and directories, and making directories: the shell's `mv`,
//! `cp`, `rm` and `mkdir`, asked for under the edit permission like any other change.

use std::collections::HashMap;
use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use super::many::Many;
use super::target::{self, Target};
use super::{Check, Output, Refusal, Spec, Tool, ToolCx};
use crate::config::limits;
use crate::data;
use crate::io::{self, fs::Kind};
use crate::permission::catalog;
use crate::text::template::fill;

static SPEC: LazyLock<Spec> = super::spec!("fs");

static TEXT: LazyLock<Text> = data::load!("tools/fs.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    too_many: String,
    link: String,
    holds_cwd: String,
    is_file: String,
    full_dir: String,
    no_to: String,
    no_name: String,
    into_file: String,
    exists: String,
    into_itself: String,
    failed: String,
    with_contents: String,
    verbs: HashMap<Action, Verbs>,
}

#[derive(Deserialize)]
pub struct Args {
    action: Action,
    path: Many<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    recursive: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
enum Action {
    Move,
    Copy,
    Delete,
    Mkdir,
}

/// How the permission card asks an action, and how the result reports it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Verbs {
    ask: String,
    done: String,
}

impl Action {
    fn verbs(self) -> &'static Verbs {
        &TEXT.verbs[&self]
    }
}

/// One path to act on, and where it goes for a move or copy.
struct Step {
    from: Target,
    to: Option<Target>,
    dir: bool,
}

pub struct Fs;

impl Tool for Fs {
    type Args = Args;

    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Self::Args, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let steps = plan(cx, args)?;
        let mut asks = Vec::new();
        let mut preview = String::new();
        for step in &steps {
            // A copy only reads its source, so copying a secret asks as reading one does.
            let source = match args.action {
                Action::Copy => catalog::READ.name,
                _ => catalog::EDIT.name,
            };
            asks.extend(target::asks(source, &step.from));
            if let Some(to) = &step.to {
                asks.extend(target::asks(catalog::EDIT.name, to));
            }
            preview.push_str(&format!("{} {}\n", args.action.verbs().ask, what(step)));
        }
        Ok(Check::new(asks).with_preview(preview.trim_end().to_owned()))
    }

    fn run<'a>(
        &'a self,
        args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            // Planned again: the files may have changed while the user was asked.
            let steps = plan(cx, &args)?;
            let verbs = args.action.verbs();
            let mut lines = Vec::new();
            let mut done = 0;
            for step in &steps {
                let _guard = cx.files().lock(&step.from.path).await;
                match act(args.action, step) {
                    Ok(()) => {
                        done += 1;
                        lines.push(format!("{} {}.", verbs.done, what(step)));
                    }
                    Err(e) => lines.push(fill(
                        &TEXT.failed,
                        &[
                            ("verb", &verbs.ask),
                            ("path", &step.from.shown),
                            ("error", &e.to_string()),
                        ],
                    )),
                }
            }
            let content = lines.join("\n");
            if done == 0 {
                return Err(content);
            }
            Ok(Output::ok(content).with_details(json!({ "paths": done })))
        })
    }
}

fn act(action: Action, step: &Step) -> std::io::Result<()> {
    let from = &step.from.path;
    match (action, &step.to) {
        (Action::Move, Some(to)) => io::fs::rename(from, &to.path),
        (Action::Copy, Some(to)) => io::fs::copy_all(from, &to.path),
        (Action::Delete, _) => io::fs::remove_all(from),
        (Action::Mkdir, _) => io::fs::make_dir(from),
        (Action::Move | Action::Copy, None) => {
            unreachable!("a move or copy is planned with a `to`")
        }
    }
}

/// What a step touches, after its verb: `src/a.rs → src/b.rs`, or `d and everything in it`.
fn what(step: &Step) -> String {
    let from = &step.from.shown;
    match &step.to {
        Some(to) => format!("{from} → {}", to.shown),
        None if step.dir => fill(&TEXT.with_contents, &[("path", from)]),
        None => from.clone(),
    }
}

/// Each path resolved and checked, or why the call cannot be done, in words for the model.
fn plan(cx: &ToolCx<'_>, args: &Args) -> Result<Vec<Step>, String> {
    let paths = &cx.app.paths;
    let given = args.path.all();
    let max = limits::get().tools.fs_batch_max.value;
    if given.len() > max {
        return Err(fill(&TEXT.too_many, &[("max", &max.to_string())]));
    }
    let project = io::fs::canonical(&paths.project);
    let mut steps = Vec::new();
    for raw in given {
        let from = target::resolve(paths, raw)?;
        let path = [("path", from.shown.as_str())];
        if from.link {
            return Err(fill(&TEXT.link, &path));
        }
        if project.starts_with(&from.path) {
            return Err(fill(&TEXT.holds_cwd, &path));
        }
        let kind = from.kind()?;
        if args.action != Action::Mkdir && kind == Kind::Missing {
            return Err(fill(&target::TEXT.missing, &path));
        }
        match args.action {
            Action::Mkdir if kind == Kind::File => {
                return Err(fill(&TEXT.is_file, &path));
            }
            Action::Delete if kind == Kind::Dir && !args.recursive => {
                let entries = io::fs::list_dir(&from.path).map_or(0, |e| e.len());
                if entries > 0 {
                    return Err(fill(
                        &TEXT.full_dir,
                        &[("path", &from.shown), ("entries", &entries.to_string())],
                    ));
                }
            }
            _ => {}
        }
        let to = match args.action {
            Action::Move | Action::Copy => Some(destination(cx, args, &from, given.len())?),
            Action::Delete | Action::Mkdir => None,
        };
        // Only a delete says it takes a directory's contents with it.
        let dir = args.action == Action::Delete && kind == Kind::Dir;
        steps.push(Step { from, to, dir });
    }
    Ok(steps)
}

/// Where `from` goes: into `to` when that is a directory, as `mv` does, else `to` itself.
fn destination(
    cx: &ToolCx<'_>,
    args: &Args,
    from: &Target,
    count: usize,
) -> Result<Target, String> {
    let paths = &cx.app.paths;
    let raw = args.to.as_deref().ok_or_else(|| TEXT.no_to.clone())?;
    let to = target::resolve(paths, raw)?;
    let to = if to.kind()? == Kind::Dir {
        let name = from
            .path
            .file_name()
            .ok_or_else(|| fill(&TEXT.no_name, &[("path", &from.shown)]))?;
        target::resolve(paths, &to.path.join(name).to_string_lossy())?
    } else if count > 1 {
        return Err(fill(&TEXT.into_file, &[("path", &to.shown)]));
    } else {
        to
    };
    if to.kind()? != Kind::Missing {
        return Err(fill(&TEXT.exists, &[("path", &to.shown)]));
    }
    if to.path.starts_with(&from.path) {
        return Err(fill(&TEXT.into_itself, &[("path", &from.shown)]));
    }
    Ok(to)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::tools::testing::Project;

    async fn fs(project: &Project, args: Value) -> Output {
        project.run(&Fs, args).await
    }

    fn exists(project: &Project, path: &str) -> bool {
        io::fs::kind(&project.app.paths.project.join(path)).unwrap() != Kind::Missing
    }

    #[test]
    fn every_action_has_its_verbs() {
        for action in [Action::Move, Action::Copy, Action::Delete, Action::Mkdir] {
            assert!(!action.verbs().ask.is_empty(), "{action:?}");
        }
    }

    #[tokio::test]
    async fn moves_into_a_directory_copies_and_makes_one() {
        let project = Project::new();
        project.write("a.txt", "a");
        project.write("b.txt", "b");
        let out = fs(&project, json!({"action": "mkdir", "path": "out/deep"})).await;
        assert_eq!(out.content, "Made out/deep.");
        let out = fs(
            &project,
            json!({"action": "move", "path": ["a.txt", "b.txt"], "to": "out"}),
        )
        .await;
        assert_eq!(
            out.content,
            "Moved a.txt → out/a.txt.\nMoved b.txt → out/b.txt."
        );
        assert!(!exists(&project, "a.txt") && exists(&project, "out/b.txt"));
        let out = fs(
            &project,
            json!({"action": "copy", "path": "out", "to": "copy"}),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(exists(&project, "copy/deep") && exists(&project, "copy/a.txt"));
    }

    #[tokio::test]
    async fn deleting_a_full_directory_needs_recursive() {
        let project = Project::new();
        project.write("d/x.txt", "x");
        let out = fs(&project, json!({"action": "delete", "path": "d"})).await;
        assert!(
            out.is_error && out.content.contains("recursive"),
            "{}",
            out.content
        );
        let out = fs(
            &project,
            json!({"action": "delete", "path": "d", "recursive": true}),
        )
        .await;
        assert_eq!(out.content, "Deleted d and everything in it.");
        assert!(!exists(&project, "d"));
    }

    #[tokio::test]
    async fn refuses_to_overwrite_nest_or_touch_the_working_directory() {
        let project = Project::new();
        project.write("a.txt", "a");
        project.write("b.txt", "b");
        project.write("d/x.txt", "x");
        let refused = |args: Value| {
            let project = &project;
            async move { fs(project, args).await.content }
        };
        assert!(
            refused(json!({"action": "move", "path": "a.txt", "to": "b.txt"}))
                .await
                .contains("already exists")
        );
        assert!(
            refused(json!({"action": "copy", "path": "d", "to": "d/inner"}))
                .await
                .contains("inside itself")
        );
        assert!(
            refused(json!({"action": "delete", "path": ".", "recursive": true}))
                .await
                .contains("working directory")
        );
        assert!(
            refused(json!({"action": "move", "path": ["a.txt", "b.txt"], "to": "c.txt"}))
                .await
                .contains("not a directory")
        );
        assert!(
            refused(json!({"action": "move", "path": "a.txt"}))
                .await
                .contains("`to`")
        );
        assert!(exists(&project, "a.txt") && exists(&project, "d/x.txt"));
    }

    #[test]
    fn asks_to_edit_what_changes_and_to_read_what_a_copy_reads() {
        let project = Project::new();
        project.write("a.txt", "a");
        let check = |args: Value| project.check(&Fs, &args).ok().unwrap().asks;
        let asks = check(json!({"action": "move", "path": "a.txt", "to": "b.txt"}));
        let perms: Vec<(&str, &str)> = asks
            .iter()
            .map(|a| (a.permission.as_str(), a.patterns[0].as_str()))
            .collect();
        assert_eq!(perms, [("edit", "a.txt"), ("edit", "b.txt")]);
        let asks = check(json!({"action": "copy", "path": "a.txt", "to": "b.txt"}));
        assert_eq!(asks[0].permission, "read");
    }
}
