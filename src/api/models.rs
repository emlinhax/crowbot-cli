//! The models crowbot offers, in the shape `GET /v1/models` returns them.

use serde::{Deserialize, Serialize};

use crate::api::{Api, Call};
use crate::config::limits;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    pub context_window: u64,
    pub max_output_tokens: u64,
    pub pricing: Pricing,
    #[serde(default)]
    pub capabilities: Capabilities,
}

/// USD per million tokens.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pricing {
    pub input_per_1m_usd: f64,
    #[serde(default)]
    pub cached_input_per_1m_usd: f64,
    pub output_per_1m_usd: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Capabilities {
    pub streaming: bool,
    pub reasoning: bool,
    pub tool_calling: bool,
    pub temperature: bool,
    pub caching: bool,
}

impl Pricing {
    /// What these token counts cost; tokens times USD-per-million is exactly micro-dollars.
    pub fn cost_micros(&self, uncached: u64, cached: u64, output: u64) -> u64 {
        let micros = uncached as f64 * self.input_per_1m_usd
            + cached as f64 * self.cached_input_per_1m_usd
            + output as f64 * self.output_per_1m_usd;
        micros.round() as u64
    }
}

impl Model {
    /// The effort a request to this model carries: none for one that does not reason, which is
    /// what gets sent, recorded and shown.
    pub fn effort<'a>(&self, requested: Option<&'a str>) -> Option<&'a str> {
        requested.filter(|_| self.capabilities.reasoning)
    }
}

/// The reply to `GET /v1/models`, also the format of data/models.json.
#[derive(Serialize, Deserialize)]
pub struct Listing {
    pub data: Vec<Model>,
}

/// The live list; `Err` says why it could not be had, an empty list included.
pub async fn fetch(api: &Api) -> Result<Vec<Model>, String> {
    let timeout = limits::get().models.fetch_timeout_ms.ms();
    let resp = api
        .call("models", Call::default(), timeout)
        .await
        .map_err(|e| e.to_string())?;
    let listing: Listing =
        serde_json::from_slice(&resp.body).map_err(|e| format!("unreadable model list: {e}"))?;
    if listing.data.is_empty() {
        return Err("crowbot returned an empty model list".into());
    }
    Ok(listing.data)
}
