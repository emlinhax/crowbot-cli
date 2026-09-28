use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

const MODELS: &str = include_str!("../fixtures/api/models.json");

/// A local stand-in for crowbot that serves fixtures and records every path it was asked for.
pub struct Fake {
    pub url: String,
    hits: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    pub async fn start() -> Self {
        let hits = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/models", get(models))
            .layer(middleware::from_fn_with_state(hits.clone(), record));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { url, hits }
    }

    pub fn hits(&self, path: &str) -> usize {
        self.hits
            .lock()
            .unwrap()
            .iter()
            .filter(|p| *p == path)
            .count()
    }
}

async fn record(State(hits): State<Arc<Mutex<Vec<String>>>>, req: Request, next: Next) -> Response {
    hits.lock().unwrap().push(req.uri().path().to_owned());
    next.run(req).await
}

async fn models() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], MODELS)
}
