//! What in-crate tests share: a local server for a fake service, and a model to run with.
//! CEILING: crowbot is bin-only, so tests/e2e keeps its own twins of the run harness's SSE turns
//! (agent/run.rs) and the fake forum's replies (forums/mobiquo.rs); the upgrade is a `src/lib.rs`
//! or a test-support crate once a third copy appears.

use axum::Router;

use crate::api::models::{Capabilities, Model, Pricing};

/// Serves the router `routes` builds, on a free local port; `routes` gets the server's own
/// address, for replies that point back at it. Returns that address.
pub async fn serve(routes: impl FnOnce(&str) -> Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = routes(&base);
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    base
}

/// A model with round prices ($6 in, $0.60 cached, $10 out per 1M) that reasons when asked.
pub fn model(reasoning: bool) -> Model {
    Model {
        id: "m".into(),
        display_name: String::new(),
        description: String::new(),
        context_window: 100_000,
        max_output_tokens: 1000,
        pricing: Pricing {
            input_per_1m_usd: 6.0,
            cached_input_per_1m_usd: 0.6,
            output_per_1m_usd: 10.0,
        },
        capabilities: Capabilities {
            reasoning,
            ..Capabilities::default()
        },
    }
}
