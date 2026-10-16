use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::Duration;

use serde::Deserialize;

use crate::api::endpoints;
use crate::config::limits;
use crate::conversation::error::ErrorInfo;
use crate::data;
use crate::io::http::{HttpError, Response};
use crate::text::template;

static CATALOG: LazyLock<BTreeMap<String, Entry>> = data::load!("errors.toml");

/// How the client treats one `error.type`; see data/errors.toml.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub title: String,
    /// What to do about it; `{api}` and `{chat}` name crowbot's sites (endpoints.toml).
    #[serde(default)]
    hint: Option<String>,
    /// Whether it is worth trying again; `None` leaves that to the status.
    #[serde(default)]
    pub retry: Option<bool>,
}

/// How an error reads on one line: its kind's title, then what crowbot said.
pub fn headline(info: &ErrorInfo) -> String {
    format!("{}: {info}", Entry::of(info).title)
}

/// How a failed step reads where there is room for one notice: an error from crowbot gets its
/// headline and hint, anything else its chain of causes.
pub fn said(e: &anyhow::Error) -> String {
    let Some(api) = e.chain().find_map(|c| c.downcast_ref::<ApiError>()) else {
        return format!("{e:#}");
    };
    let mut out = headline(&api.info);
    if let Some(hint) = Entry::of(&api.info).hint() {
        out.push_str(". ");
        out.push_str(&hint);
    }
    out
}

impl Entry {
    /// How the client treats `info`'s kind; an unknown kind reads as `unknown`.
    pub fn of(info: &ErrorInfo) -> &'static Self {
        CATALOG
            .get(&info.kind)
            .unwrap_or_else(|| &CATALOG["unknown"])
    }

    /// The hint, naming the sites this build talks to.
    pub fn hint(&self) -> Option<String> {
        let sites = [
            ("api", endpoints::origin("api")),
            ("chat", endpoints::origin("chat")),
        ];
        let sites: Vec<(&str, &str)> = sites.iter().map(|(k, v)| (*k, v.as_str())).collect();
        self.hint
            .as_deref()
            .map(|hint| template::fill(hint, &sites))
    }
}

/// A reply's body read as JSON; one that is not what was expected is a `bad_response`.
pub fn json<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|e| ErrorInfo::local("bad_response", e.to_string()).into())
}

#[derive(Debug, thiserror::Error)]
#[error("{info}")]
pub struct ApiError {
    pub info: ErrorInfo,
    pub retry_after: Option<Duration>,
}

#[derive(Deserialize)]
struct Envelope {
    error: ErrorInfo,
}

impl ApiError {
    pub fn from_response(resp: &Response) -> Self {
        let mut info = match serde_json::from_slice::<Envelope>(&resp.body) {
            Ok(envelope) => envelope.error,
            Err(_) => ErrorInfo::local(
                "http_error",
                String::from_utf8_lossy(&resp.body)
                    .chars()
                    .take(limits::get().http.error_text_chars.value)
                    .collect::<String>(),
            ),
        };
        info.status = Some(resp.head.status);
        info.request_id.clone_from(&resp.head.request_id);
        Self {
            info,
            retry_after: resp.head.retry_after,
        }
    }

    pub fn not_logged_in() -> Self {
        ErrorInfo::local("not_logged_in", "no crowbot key on this machine").into()
    }
}

impl From<ErrorInfo> for ApiError {
    fn from(info: ErrorInfo) -> Self {
        Self {
            info,
            retry_after: None,
        }
    }
}

impl From<HttpError> for ApiError {
    fn from(e: HttpError) -> Self {
        let kind = match e {
            HttpError::Timeout => "timeout",
            HttpError::Connect(_) | HttpError::Other(_) => "network",
        };
        ErrorInfo::local(kind, e.to_string()).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_hint_fills_its_placeholders() {
        for (kind, entry) in CATALOG.iter() {
            if let Some(hint) = entry.hint() {
                assert!(!hint.contains('{'), "{kind}: {hint}");
            }
        }
        let low = Entry::of(&ErrorInfo::local("insufficient_balance", ""));
        assert_eq!(
            low.hint().as_deref(),
            Some("Top up at https://chat.crowbot.sh.")
        );
    }
    use crate::io::http::Head;

    fn response(status: u16, body: &str) -> Response {
        Response {
            head: Head {
                status,
                request_id: Some("req_1".into()),
                retry_after: Some(Duration::from_secs(3)),
            },
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn said_names_the_kind_and_its_hint_and_falls_back_to_the_causes() {
        let refused = ApiError::from(ErrorInfo::local("key_unknown", "Invalid API key"));
        let said = said(&anyhow::Error::new(refused).context("checking that key with crowbot"));
        assert!(
            said.starts_with("crowbot does not know that key: Invalid API key (key_unknown)."),
            "{said}"
        );
        assert!(said.ends_with("or pair this device instead."), "{said}");
        let other = anyhow::anyhow!("disk full").context("saving the key");
        assert_eq!(super::said(&other), "saving the key: disk full");
    }

    #[test]
    fn catalog_parses_and_has_a_fallback() {
        assert!(CATALOG.contains_key("unknown"));
        assert_eq!(
            Entry::of(&ErrorInfo::local("no_such_kind", "x")).title,
            CATALOG["unknown"].title
        );
    }

    #[test]
    fn parses_crowbot_envelope() {
        let err = ApiError::from_response(&response(
            402,
            r#"{"error":{"message":"top up","type":"insufficient_balance"}}"#,
        ));
        assert_eq!(err.info.kind, "insufficient_balance");
        assert_eq!(err.info.message, "top up");
        assert_eq!(err.info.status, Some(402));
        assert_eq!(err.info.request_id.as_deref(), Some("req_1"));
        assert_eq!(err.retry_after, Some(Duration::from_secs(3)));
        assert!(err.to_string().contains("request req_1"));
    }

    #[test]
    fn non_json_body_still_reports() {
        let err = ApiError::from_response(&response(502, "Bad Gateway"));
        assert_eq!(err.info.kind, "http_error");
        assert_eq!(err.info.message, "Bad Gateway");
    }
}
