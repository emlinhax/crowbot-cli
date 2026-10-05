//! Moving, copying and deleting files and directories, and making directories: the shell's `mv`,
//! `cp`, `rm` and `mkdir`, asked for under the edit permission like any other change.

use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

use super::many::Many;
use super::permissions;
use super::target::{self, Target};
use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail};
use crate::io::{self, fs::Kind};
use crate::limits;

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "fs",
        include_str!("../../data/tools/fs.md"),
        include_str!("../../data/tools/fs.schema.json"),
    )
});

#[derive(Deserialize)]
struct Args {
    action: Action,
    path: Many<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    recursive: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Action {
    Move,
    Copy,
    Delete,
    Mkdir,
}

impl Action {
    /// How the permission card asks it, and how the result reports it.
    fn verbs(self) -> (&'static str, &'static str) {
        match self {
            Self::Move => ("move", "Moved"),
            Self::Copy => ("copy", "Copied"),
            Self::Delete => ("delete", "Deleted"),
            Self::Mkdir => ("make", "Made"),
        }
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
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args: Args = parse(args)?;
        let steps = plan(cx, &args).map_err(Refusal::Refused)?;
        let mut asks = Vec::new();
        let mut preview = String::new();
        for step in &steps {
            // A copy only reads its source, so copying a secret asks as reading one does.
            let source = match args.action {
                Action::Copy => permissions::READ.name,
                _ => permissions::EDIT.name,
            };
            asks.extend(target::asks(source, &step.from));
            if let Some(to) = &step.to {
                asks.extend(target::asks(permissions::EDIT.name, to));
            }
            preview.push_str(&format!("{} {}\n", args.action.verbs().0, what(step)));
        }
        Ok(Check::new(asks).with_preview(preview.trim_end().to_owned()))
    }

    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let args: Args = match parse_or_fail(&args) {
                Ok(args) => args,
                Err(out) => return out,
            };
            // Planned again: the files may have changed while the user was asked.
            let steps = match plan(cx, &args) {
                Ok(steps) => steps,
                Err(why) => return Output::error(why),
            };
            let (verb, done_verb) = args.action.verbs();
            let mut lines = Vec::new();
            let mut done = 0;
            for step in &steps {
                let _guard = cx.files().lock(&step.from.path).await;
                match act(args.action, step) {
                    Ok(()) => {
                        done += 1;
                        lines.push(format!("{done_verb} {}.", what(step)));
                    }
                    Err(e) => lines.push(format!("Could not {verb} {}: {e}", step.from.shown)),
                }
            }
            let content = lines.join("\n");
            if done == 0 {
                return Output::error(content);
            }
            Output::ok(content).with_details(json!({ "paths": done }))
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
        None if step.dir => format!("{from} and everything in it"),
        None => from.clone(),
    }
}

/// Each path resolved and checked, or why the call cannot be done, in words for the model.
fn plan(cx: &ToolCx<'_>, args: &Args) -> Result<Vec<Step>, String> {
    let paths = &cx.app.paths;
    let given: Vec<String> = match &args.path {
        Many::One(path) => vec![path.clone()],
        Many::Set(paths) => paths.clone(),
    };
    let max = limits::get().tools.fs_batch_max.value;
    if given.len() > max {
        return Err(format!("At most {max} paths in one call."));
    }
    let project = io::fs::canonical(&paths.project);
    let mut steps = Vec::new();
    for raw in &given {
        let from = target::resolve(paths, raw)?;
        if from.link {
            return Err(format!(
                "{} is a symbolic link; handle links in the shell.",
                from.shown
            ));
        }
        if project.starts_with(&from.path) {
            return Err(format!(
                "{} holds the working directory; it stays where it is.",
                from.shown
            ));
        }
        let kind = from.kind()?;
        if args.action != Action::Mkdir && kind == Kind::Missing {
            return Err(format!("{} does not exist.", from.shown));
        }
        match args.action {
            Action::Mkdir if kind == Kind::File => {
                return Err(format!("{} is a file.", from.shown));
            }
            Action::Delete if kind == Kind::Dir && !args.recursive => {
                let entries = io::fs::list_dir(&from.path).map_or(0, |e| e.len());
                if entries > 0 {
                    return Err(format!(
                        "{} is a directory holding {entries} entries; pass `recursive: true` \
                         to delete it and everything in it.",
                        from.shown
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
    let raw = args
        .to
        .as_deref()
        .ok_or("`to` is required to move or copy.")?;
    let to = target::resolve(paths, raw)?;
    let to = if to.kind()? == Kind::Dir {
        let name = from
            .path
            .file_name()
            .ok_or_else(|| format!("{} has no name to keep.", from.shown))?;
        target::resolve(paths, &to.path.join(name).to_string_lossy())?
    } else if count > 1 {
        return Err(format!(
            "{} is not a directory; several paths can only go into one.",
            to.shown
        ));
    } else {
        to
    };
    if to.kind()? != Kind::Missing {
        return Err(format!(
            "{} already exists; delete it first or pick another name.",
            to.shown
        ));
    }
    if to.path.starts_with(&from.path) {
        return Err(format!("{} cannot go inside itself.", from.shown));
    }
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::testing::Project;

    async fn fs(project: &Project, args: Value) -> Output {
        Fs.run(args, &project.cx()).await
    }

    fn exists(project: &Project, path: &str) -> bool {
        io::fs::kind(&project.app.paths.project.join(path)).unwrap() != Kind::Missing
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
        let check = |args: Value| Fs.check(&args, &project.cx()).ok().unwrap().asks;
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
