use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::Duration;

use serde::Deserialize;

use crate::config::limits;
use crate::conversation::error::ErrorInfo;
use crate::data;
use crate::io::http::{HttpError, Response};

static CATALOG: LazyLock<BTreeMap<String, Entry>> = data::load!("errors.toml");

/// How the client treats one `error.type`; see data/errors.toml.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub title: String,
    #[serde(default)]
    pub hint: Option<String>,
    /// Whether it is worth trying again; `None` leaves that to the status.
    #[serde(default)]
    pub retry: Option<bool>,
}

impl Entry {
    /// How the client treats `info`'s kind; an unknown kind reads as `unknown`.
    pub fn of(info: &ErrorInfo) -> &'static Self {
        CATALOG
            .get(&info.kind)
            .unwrap_or_else(|| &CATALOG["unknown"])
    }
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
