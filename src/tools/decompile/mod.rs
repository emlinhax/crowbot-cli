//! The decompile tool: runs kuna to turn a compiled binary into readable C pseudocode, so a model
//! reverses from pseudocode instead of hand-reading disassembly. kuna is a separate program; when
//! it is not installed the model is told how to get it (and to fall back to objdump).

mod cache;

use std::sync::LazyLock;
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx, target};
use crate::config::limits;
use crate::data;
use crate::io::proc::{self, Program};
use crate::permission::catalog;
use crate::text::template::fill;

static SPEC: LazyLock<Spec> = super::spec!("decompile");

static TEXT: LazyLock<Text> = data::load!("tools/decompile.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    no_kuna: String,
    failed: String,
    no_output: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    binary: String,
    #[serde(default)]
    function: Option<String>,
}

pub struct Decompile;

impl Tool for Decompile {
    type Args = Args;

    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Self::Args, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        // Reading and analysing the binary asks what a read of it asks.
        let target = target::resolve(&cx.app.paths, &args.binary)?;
        Ok(Check::new(target::asks(catalog::READ.name, &target)))
    }

    fn run<'a>(
        &'a self,
        args: Self::Args,
        cx: &'a ToolCx<'a>,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let Some(kuna) = proc::which("kuna") else {
                return Ok(Output::error(TEXT.no_kuna.clone()));
            };
            let target = target::resolve(&cx.app.paths, &args.binary)?;
            let binary = target.path.to_string_lossy().into_owned();
            // One tool, two modes: name a function to decompile it, or list the functions first.
            let (action, argv) = match &args.function {
                Some(func) => (
                    "decompile",
                    vec!["decompile".to_owned(), binary, func.clone()],
                ),
                None => (
                    "list the functions of",
                    vec!["functions".to_owned(), binary],
                ),
            };
            let cached = cache::file(&cx.app.paths, &kuna, &target.path, &argv);
            if let Some(out) = cache::get(&cached) {
                return Ok(answer(&out, &args));
            }
            let fail = |error: &str| {
                fill(
                    &TEXT.failed,
                    &[
                        ("action", action),
                        ("binary", &target.shown),
                        ("error", error),
                    ],
                )
            };
            let captured = proc::exec(&Program {
                program: &kuna,
                args: &argv,
                cwd: &cx.app.paths.project,
                env: &[],
                unset: &[],
                timeout: Duration::from_secs(limits::get().tools.decompile_timeout_secs.value),
            })
            .await
            .map_err(|e| fail(&e.to_string()))?;
            if !captured.ok() {
                // kuna prints the real reason on stderr, then (on a bad path or argument) its whole
                // usage block; keep the reason, drop the dump.
                let stderr = String::from_utf8_lossy(&captured.stderr);
                let why = stderr.split("\nusage:").next().unwrap_or(&stderr).trim();
                return Ok(Output::error(fail(if why.is_empty() {
                    "it exited with an error"
                } else {
                    why
                })));
            }
            let out = String::from_utf8_lossy(&captured.stdout);
            if out.trim().is_empty() {
                return Ok(Output::error(fill(
                    &TEXT.no_output,
                    &[("binary", &target.shown)],
                )));
            }
            cache::put(&cached, &out);
            Ok(answer(&out, &args))
        })
    }
}

fn answer(out: &str, args: &Args) -> Output {
    Output::head(out).with_details(json!({
        "mode": if args.function.is_some() { "decompile" } else { "functions" },
    }))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;
    use crate::tools::testing::Project;

    /// True only for the real kuna decompiler: an unrelated binary of the same name (there is a
    /// `kuna` on PyPI) answers `--version` differently, so it neither drives nor fails the test.
    async fn real_kuna() -> bool {
        let Some(kuna) = proc::which("kuna") else {
            return false;
        };
        let out = proc::exec(&Program {
            program: &kuna,
            args: &["--version".to_owned()],
            cwd: Path::new("."),
            env: &[],
            unset: &[],
            timeout: Duration::from_secs(10),
        })
        .await;
        matches!(out, Ok(c) if c.ok() && String::from_utf8_lossy(&c.stdout).starts_with("kuna "))
    }

    #[tokio::test]
    async fn without_kuna_it_says_how_to_get_it_instead_of_failing_hard() {
        // The host running tests has no kuna; the tool should name the install path, as an error
        // the model can act on, not a crash.
        let project = Project::new();
        project.write("prog", "not really a binary");
        if proc::which("kuna").is_some() {
            return; // Any kuna on PATH would be run instead of the no-kuna branch under test.
        }
        let out = project.run(&Decompile, json!({ "binary": "prog" })).await;
        assert!(out.is_error);
        assert!(out.content.contains("kuna"), "{}", out.content);
        assert!(out.content.contains("PATH"), "{}", out.content);
    }

    #[tokio::test]
    async fn with_kuna_it_lists_functions_then_decompiles_one() {
        // Runs only where the real kuna is installed (CI has objdump, not kuna); proves the two
        // modes against the committed stripped `vault` crackme.
        if !real_kuna().await {
            return;
        }
        let project = Project::new();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/projects/crackme/vault");
        let bytes = crate::io::fs::read_bytes(&fixture).unwrap();
        crate::io::fs::write_atomic(
            &project.app.paths.project.join("vault"),
            &bytes,
            crate::io::fs::Access::Shared,
        )
        .unwrap();
        let listed = project.run(&Decompile, json!({ "binary": "vault" })).await;
        assert!(!listed.is_error, "{}", listed.content);
        assert!(listed.content.contains("main"), "{}", listed.content);
        let out = project
            .run(&Decompile, json!({ "binary": "vault", "function": "main" }))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("main("), "{}", out.content);
        // Both answers are kept, so asking again does not run kuna again.
        let kept = crate::io::fs::list_dir(&project.app.paths.decompile_cache()).unwrap();
        assert_eq!(kept.len(), 2);
        let again = project
            .run(&Decompile, json!({ "binary": "vault", "function": "main" }))
            .await;
        assert_eq!(again.content, out.content);
    }
}
