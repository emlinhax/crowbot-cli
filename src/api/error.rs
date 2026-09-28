use std::fmt;

use serde::Deserialize;

use crate::io::http::{HttpError, Response};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error(transparent)]
    Http(#[from] HttpError),
    /// crowbot's `{"error": {"message", "type"}}` envelope; `kind` is the field to branch on.
    #[error("{}", StatusText { status: *status, kind, message, request_id: request_id.as_deref() })]
    Status {
        status: u16,
        kind: String,
        message: String,
        request_id: Option<String>,
    },
}

/// The request id is what crowbot support needs, so it rides along whenever there is one.
struct StatusText<'a> {
    status: u16,
    kind: &'a str,
    message: &'a str,
    request_id: Option<&'a str>,
}

impl fmt::Display for StatusText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}, HTTP {}", self.message, self.kind, self.status)?;
        if let Some(id) = self.request_id {
            write!(f, ", request {id}")?;
        }
        write!(f, ")")
    }
}

#[derive(Deserialize)]
struct Envelope {
    error: Body,
}

#[derive(Deserialize)]
struct Body {
    #[serde(default)]
    message: String,
    #[serde(rename = "type", default)]
    kind: String,
}

impl ApiError {
    pub fn from_response(resp: &Response) -> Self {
        let (kind, message) = match serde_json::from_slice::<Envelope>(&resp.body) {
            Ok(Envelope { error }) => (error.kind, error.message),
            Err(_) => (
                "http_error".to_owned(),
                String::from_utf8_lossy(&resp.body)
                    .chars()
                    .take(200)
                    .collect(),
            ),
        };
        Self::Status {
            status: resp.status,
            kind,
            message,
            request_id: resp.request_id.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, body: &str) -> Response {
        Response {
            status,
            request_id: Some("req_1".into()),
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn parses_crowbot_envelope() {
        let err = ApiError::from_response(&response(
            402,
            r#"{"error":{"message":"top up","type":"insufficient_balance"}}"#,
        ));
        let ApiError::Status {
            kind,
            message,
            request_id,
            ..
        } = err
        else {
            panic!("expected a status error");
        };
        assert_eq!(kind, "insufficient_balance");
        assert_eq!(message, "top up");
        assert_eq!(request_id.as_deref(), Some("req_1"));
    }

    #[test]
    fn non_json_body_still_reports() {
        let ApiError::Status { kind, message, .. } =
            ApiError::from_response(&response(502, "Bad Gateway"))
        else {
            panic!("expected a status error");
        };
        assert_eq!(kind, "http_error");
        assert_eq!(message, "Bad Gateway");
    }
}
