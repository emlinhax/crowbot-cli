//! The error that ended a reply, as stored in sessions and shown to the user.

use std::fmt;

use serde::{Deserialize, Serialize};

/// An error as stored in sessions and shown to the user. `kind` is crowbot's `error.type`, or
/// one of our own for failures that never reached crowbot (data/errors.toml lists both).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorInfo {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub request_id: Option<String>,
}

impl ErrorInfo {
    pub fn local(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.to_owned(),
            message: message.into(),
            status: None,
            request_id: None,
        }
    }
}

impl fmt::Display for ErrorInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}", self.message, self.kind)?;
        if let Some(status) = self.status {
            write!(f, ", HTTP {status}")?;
        }
        // The request id is what crowbot support needs to find the call.
        if let Some(id) = &self.request_id {
            write!(f, ", request {id}")?;
        }
        write!(f, ")")
    }
}
