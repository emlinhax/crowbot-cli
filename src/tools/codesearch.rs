//! crowbot's server-side code search; the description and schema are crowbot's own, verbatim.

use std::fmt::Write as _;
use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::Value;

use super::permissions;
use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail};
use crate::api::Call;
use crate::limits;
use crate::permission::gate::Ask;
use crate::text::shorten;

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "codesearch",
        include_str!("../../data/tools/codesearch.md"),
        include_str!("../../data/tools/codesearch.schema.json"),
    )
});

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
            permissions::CODESEARCH.name,
            args.query,
        )]))
    }

    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            if let Err(out) = parse_or_fail::<Args>(&args) {
                return out;
            }
            // The schema is crowbot's, so the arguments go through untouched.
            let resp = cx
                .app
                .api
                .call(
                    "codesearch",
                    Call {
                        body: Some(&args),
                        ..Call::default()
                    },
                    limits::get().http.request_timeout_ms.ms(),
                )
                .await;
            let resp = match resp {
                Ok(resp) => resp,
                Err(e) => return Output::error(format!("{}: {}", e.info.entry().title, e.info)),
            };
            let reply: Reply = match serde_json::from_slice(&resp.body) {
                Ok(reply) => reply,
                Err(e) => return Output::error(format!("Unreadable search results: {e}")),
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
