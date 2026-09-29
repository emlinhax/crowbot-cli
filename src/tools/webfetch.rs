use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

use super::truncate::{self, Keep};
use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail};
use crate::io::fetch::FetchError;
use crate::limits;
use crate::permission::gate::Ask;
use crate::text::template::fill;

const TEXT_SRC: &str = include_str!("../../data/tools/webfetch.toml");

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "webfetch",
        include_str!("../../data/tools/webfetch.md"),
        include_str!("../../data/tools/webfetch.schema.json"),
    )
});

static TEXT: LazyLock<Text> = LazyLock::new(|| {
    toml::from_str(TEXT_SRC).expect("data/tools/webfetch.toml is checked by tests")
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    failed: String,
    status: String,
    firewall: String,
    challenge: String,
    browser: String,
    slow: String,
    cut: String,
}

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

/// Why a fetch brought back no page.
fn failure(url: &str, error: &FetchError) -> String {
    match error {
        FetchError::Challenge { .. } => fill(
            &TEXT.challenge,
            &[("url", url), ("error", &error.to_string())],
        ),
        FetchError::NeedsBrowser { .. } => fill(&TEXT.browser, &[("url", url)]),
        FetchError::Timeout => fill(&TEXT.slow, &[("url", url)]),
        FetchError::Transport(why) => fill(&TEXT.failed, &[("url", url), ("error", why)]),
    }
}

/// Why a page that did arrive is not worth reading; `None` when it is.
fn refusal(url: &str, status: u16, firewall: bool) -> Option<String> {
    if status < 400 {
        return None;
    }
    let text = if firewall {
        &TEXT.firewall
    } else {
        &TEXT.status
    };
    Some(fill(text, &[("url", url), ("status", &status.to_string())]))
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
        Ok(Check::new(vec![Ask::new("webfetch", host)]))
    }

    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let args: Args = match parse_or_fail(&args) {
                Ok(args) => args,
                Err(out) => return out,
            };
            let limits = &limits::get().tools;
            let url = args.url.trim().to_owned();
            let page = match cx.app.fetch.get(&url).await {
                Ok(page) => page,
                Err(e) => return Output::error(failure(&url, &e)),
            };
            if let Some(why) = refusal(&url, page.status, page.firewall) {
                return Output::error(why);
            }
            let mut body = page.body;
            body.truncate(limits.webfetch_max_bytes.value);
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
                let (kept, total) = (cut.kept_lines.to_string(), cut.total_lines.to_string());
                content.push_str("\n\n");
                content.push_str(&fill(&TEXT.cut, &[("kept", &kept), ("total", &total)]));
            }
            Output::ok(content).with_details(json!({
                "url": url,
                "final_url": page.url,
                "status": page.status,
                "fingerprint": page.fingerprint,
            }))
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

    #[test]
    fn every_failure_says_what_happened_and_whether_to_retry() {
        let url = "https://example.com/doc";
        let challenge = FetchError::Challenge {
            tried: vec!["Chrome131".into(), "Chrome124".into()],
            last_status: 403,
        };
        let cases = [
            (challenge, "Cloudflare challenge"),
            (FetchError::NeedsBrowser { url: url.into() }, "real browser"),
            (FetchError::Timeout, "too long"),
            (
                FetchError::Transport("connection refused".into()),
                "connection refused",
            ),
        ];
        for (error, says) in cases {
            let text = failure(url, &error);
            assert!(text.contains(url), "{text}");
            assert!(text.contains(says), "{text}");
            assert!(!text.contains('{'), "unfilled placeholder: {text}");
        }
        assert!(
            failure(url, &FetchError::NeedsBrowser { url: url.into() }).contains("do not retry")
        );
    }

    #[test]
    fn a_page_that_arrived_is_read_unless_its_status_says_otherwise() {
        let url = "https://example.com";
        assert_eq!(refusal(url, 200, false), None);
        assert_eq!(
            refusal(url, 404, false).as_deref(),
            Some("https://example.com answered HTTP 404.")
        );
        assert!(refusal(url, 403, true).unwrap().contains("firewall"));
    }
}
