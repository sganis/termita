// src/main.rs
// Entry point: read config from the environment, build the router, and serve.
mod bridge;
mod serve;
mod ssh;

use std::sync::Arc;

use serve::AppState;

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let allowed: Vec<String> = std::env::var("ALLOWED_HOSTS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let app = serve::router(AppState { allowed: Arc::new(allowed) });

    let addr = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("cannot bind {addr}: {e}"));
    println!("termita web-ssh on http://{addr}");
    axum::serve(listener, app).await.expect("server error");
}
