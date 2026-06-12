// src/serve.rs
// HTTP surface: serves the embedded Svelte bundle (web/dist, baked into the binary
// at compile time by rust-embed) and upgrades /ws to a WebSocket handled by the
// bridge. SPA fallback: any unknown path returns index.html.
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{ws::WebSocketUpgrade, State},
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use rust_embed::{EmbeddedFile, RustEmbed};

use crate::bridge;

#[derive(RustEmbed)]
#[folder = "web/dist"]
struct Asset;

#[derive(Clone)]
pub struct AppState {
    pub allowed: Arc<Vec<String>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .fallback(static_handler)
        .with_state(state)
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| async move {
        bridge::handle(socket, &state.allowed).await;
    })
}

async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Asset::get(path) {
        Some(file) => serve_asset(path, file),
        // SPA fallback so deep links / reloads land on the app.
        None => match Asset::get("index.html") {
            Some(index) => serve_asset("index.html", index),
            None => (StatusCode::NOT_FOUND, "not found").into_response(),
        },
    }
}

fn serve_asset(path: &str, file: EmbeddedFile) -> Response {
    let mime = file.metadata.mimetype();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, cache_for(path))
        .body(Body::from(file.data.into_owned()))
        .unwrap()
}

// Vite emits content-hashed asset filenames, so they're safe to cache forever;
// index.html must always be revalidated.
fn cache_for(path: &str) -> &'static str {
    if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}
