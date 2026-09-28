use std::sync::LazyLock;

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

use super::truncate::{self, Keep};
use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail};
use crate::io::http::{Method, Request};
use crate::limits;
use crate::permission::gate::Ask;

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "webfetch",
        include_str!("../../data/tools/webfetch.md"),
        include_str!("../../data/tools/webfetch.schema.json"),
    )
});

#[derive(Deserialize)]
struct Args {
    url: String,
}

pub struct WebFetch;

/// The host part of an http(s) URL, which is what permission rules match.
fn host(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?;
    (!host.is_empty()).then_some(host)
}

impl Tool for WebFetch {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn check(&self, args: &Value, _cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args: Args = parse(args)?;
        let Some(host) = host(args.url.trim()) else {
            return Err(Refusal::Refused(format!(
                "{} is not an http(s) URL.",
                args.url
            )));
        };
        Ok(Check {
            asks: vec![Ask::new("webfetch", host)],
        })
    }

    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let args: Args = match parse_or_fail(&args) {
                Ok(args) => args,
                Err(out) => return out,
            };
            let limits = &limits::get().tools;
            let url = args.url.trim().to_owned();
            let opened = cx
                .app
                .http
                .open(Request {
                    method: Method::Get,
                    url: url.clone(),
                    bearer: None,
                    headers: &[("accept", "text/html, text/markdown, text/plain, */*")],
                    json: None,
                    timeout: limits.webfetch_timeout_secs.secs(),
                })
                .await;
            let mut resp = match opened {
                Ok(resp) => resp,
                Err(e) => return Output::error(format!("Could not fetch {url}: {e}")),
            };
            if resp.status >= 400 {
                return Output::error(format!("{url} answered HTTP {}.", resp.status));
            }
            let max = limits.webfetch_max_bytes.value;
            let mut body = Vec::new();
            let read = tokio::time::timeout(limits.webfetch_timeout_secs.secs(), async {
                while let Some(chunk) = resp.body.next().await {
                    match chunk {
                        Ok(chunk) => body.extend(chunk),
                        Err(e) => return Err(e),
                    }
                    if body.len() > max {
                        break;
                    }
                }
                Ok(())
            })
            .await;
            match read {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Output::error(format!("Reading {url} failed: {e}")),
                Err(_) => return Output::error(format!("{url} took too long to send its page.")),
            }
            body.truncate(max);
            let text = String::from_utf8_lossy(&body);
            let looks_html = text.trim_start().starts_with('<');
            let text = if looks_html {
                htmd::convert(&text).unwrap_or_else(|_| text.into_owned())
            } else {
                text.into_owned()
            };
            let cut = truncate::cut(
                &text,
                Keep::Head,
                limits.max_lines.value,
                limits.max_bytes.value,
            );
            let mut content = cut.text.clone();
            if cut.truncated() {
                content.push_str(&format!(
                    "\n\n[Page cut at {} of {} lines.]",
                    cut.kept_lines, cut.total_lines
                ));
            }
            Output::ok(content).with_details(json!({"url": url, "status": resp.status}))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_is_per_host() {
        assert_eq!(host("https://docs.rs/x?y=1"), Some("docs.rs"));
        assert_eq!(
            host("http://user@example.com:8080/a"),
            Some("example.com:8080")
        );
        assert_eq!(host("ftp://x"), None);
    }
}
