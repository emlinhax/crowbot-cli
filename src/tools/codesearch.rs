//! crowbot's server-side code search; the description and schema are crowbot's own, verbatim.

use std::fmt::Write as _;
use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::Value;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail};
use crate::api::Call;
use crate::api::error::Entry;
use crate::config::limits;
use crate::permission::catalog;
use crate::permission::gate::Ask;
use crate::text::shorten;

static SPEC: LazyLock<Spec> = super::spec!("codesearch");

#[derive(Deserialize)]
struct Args {
    query: String,
}

#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    results: Vec<Hit>,
    #[serde(default)]
    truncated: bool,
}

#[derive(Deserialize)]
struct Hit {
    #[serde(default)]
    repo: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    line: Option<u64>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    snippet: String,
}

pub struct CodeSearch;

impl Tool for CodeSearch {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Value, _cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args: Args = parse(args)?;
        Ok(Check::new(vec![Ask::new(
            catalog::CODESEARCH.name,
            args.query,
        )]))
    }

    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            if let Err(out) = parse_or_fail::<Args>(&args) {
                return out;
            }
            // The schema is crowbot's, so the arguments go through untouched.
            let reply = cx
                .app
                .api
                .call_json::<Reply>(
                    "codesearch",
                    Call {
                        body: Some(&args),
                        ..Call::default()
                    },
                    limits::get().http.request_timeout_ms.ms(),
                )
                .await;
            let reply = match reply {
                Ok(reply) => reply,
                Err(e) => {
                    return Output::error(format!("{}: {}", Entry::of(&e.info).title, e.info));
                }
            };
            if reply.results.is_empty() {
                return Output::ok("No results. Drop the repo filter or a term and search again.");
            }
            let width = limits::get().tools.grep_line_chars.value;
            let mut out = String::new();
            for hit in &reply.results {
                let line = hit.line.map(|l| format!(":{l}")).unwrap_or_default();
                let _ = writeln!(out, "{} {}{line}  {}", hit.repo, hit.path, hit.url);
                for snippet_line in hit.snippet.lines() {
                    let _ = writeln!(out, "    {}", shorten::line(snippet_line, width));
                }
            }
            if reply.truncated {
                out.push_str("[More results exist; narrow the query.]");
            }
            Output::ok(out.trim_end().to_owned())
        })
    }
}
