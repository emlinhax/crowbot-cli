use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

use super::permissions;
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
    redirect: String,
    invalid: String,
}

#[derive(Deserialize)]
struct Args {
    url: String,
}

pub struct WebFetch;

/// Parsed as the client will parse it, so the host that is asked for is the host fetched.
fn parse_url(raw: &str) -> Result<Url, Refusal> {
    let invalid = || Refusal::Refused(fill(&TEXT.invalid, &[("url", raw.trim())]));
    let url = Url::parse(raw.trim()).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(invalid());
    }
    Ok(url)
}

/// What permission rules match: the host, and the port when one is given.
fn host(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default().to_lowercase();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    }
}

/// A redirect is reported, not followed, so its target is asked for under its own host.
fn redirect(url: &Url, status: u16, location: Option<&str>) -> Option<String> {
    if !(300..400).contains(&status) {
        return None;
    }
    let target = url.join(location?).ok()?;
    Some(fill(
        &TEXT.redirect,
        &[("url", url.as_str()), ("location", target.as_str())],
    ))
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
        let url = parse_url(&args.url)?;
        Ok(
            Check::new(vec![Ask::new(permissions::WEBFETCH.name, host(&url))])
                .with_preview(url.as_str()),
        )
    }

    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let args: Args = match parse_or_fail(&args) {
                Ok(args) => args,
                Err(out) => return out,
            };
            let limits = &limits::get().tools;
            let parsed = match parse_url(&args.url) {
                Ok(url) => url,
                Err(why) => return Output::error(why.to_string()),
            };
            let url = parsed.as_str().to_owned();
            let page = match cx.app.fetch.get(&url).await {
                Ok(page) => page,
                Err(e) => return Output::error(failure(&url, &e)),
            };
            if let Some(moved) = redirect(&parsed, page.status, page.location.as_deref()) {
                return Output::ok(moved);
            }
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
    fn permission_is_per_host_as_the_client_reads_it() {
        let asked = |raw: &str| parse_url(raw).map(|u| host(&u)).ok();
        assert_eq!(asked("https://docs.rs/x?y=1").as_deref(), Some("docs.rs"));
        assert_eq!(
            asked("http://user@example.com:8080/a").as_deref(),
            Some("example.com:8080")
        );
        assert_eq!(asked("https://Docs.RS:443/").as_deref(), Some("docs.rs"));
        // A backslash ends the host for the client, so `docs.rs` here is only a path.
        assert_eq!(asked("https://evil\\@docs.rs/").as_deref(), Some("evil"));
        assert_eq!(asked("ftp://x"), None);
        assert_eq!(asked("not a url"), None);
    }

    #[test]
    fn a_redirect_names_its_target_instead_of_being_followed() {
        let url = Url::parse("https://docs.rs/x").unwrap();
        let text = redirect(&url, 301, Some("/y")).unwrap();
        assert!(text.contains("https://docs.rs/y"), "{text}");
        assert_eq!(redirect(&url, 200, Some("/y")), None);
        assert_eq!(redirect(&url, 302, None), None);
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
