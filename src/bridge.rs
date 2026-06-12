// src/bridge.rs
// One WebSocket connection <-> one SSH shell. Parses the client's JSON control
// frames, drives the russh session, and relays bytes both ways.
//
// Wire protocol (unchanged from the Node version, so the Svelte frontend is
// untouched):
//   client -> server (text JSON): {t:"connect",host,user,port,password,cols,rows}
//                                 {t:"in",d} | {t:"sz",cols,rows}
//   server -> client: {t:"ready"} | {t:"err",reason} | {t:"status",phase} (text),
//                     raw remote-shell bytes (binary).
use axum::extract::ws::{Message, WebSocket};
use russh::client::Msg;
use russh::{Channel, ChannelMsg};
use serde::Deserialize;

use crate::ssh;

#[derive(Deserialize)]
struct Connect {
    #[serde(default)]
    host: String,
    #[serde(default)]
    user: String,
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default)]
    password: String,
    #[serde(default = "default_cols")]
    cols: u32,
    #[serde(default = "default_rows")]
    rows: u32,
}
fn default_port() -> u16 { 22 }
fn default_cols() -> u32 { 80 }
fn default_rows() -> u32 { 24 }

pub async fn handle(mut ws: WebSocket, allowed: &[String]) {
    // 1) Wait for the opening `connect` frame.
    let conn = match recv_connect(&mut ws).await {
        Some(c) => c,
        None => return,
    };

    let host = conn.host.trim().to_string();
    let user = conn.user.trim().to_string();
    if host.is_empty() || user.is_empty() {
        return err(&mut ws, "Host and username are required.").await;
    }
    if !allowed.is_empty() && !allowed.iter().any(|h| h == &host) {
        return err(&mut ws, &format!("Host not allowed: {host}")).await;
    }

    // 2) Connect + authenticate (russh bundles transport, auth, and shell setup).
    let params = ssh::Params {
        host,
        port: conn.port,
        user,
        password: conn.password,
        cols: conn.cols,
        rows: conn.rows,
    };
    let shell = match ssh::connect(&params).await {
        Ok(s) => s,
        Err(reason) => return err(&mut ws, &reason).await,
    };

    // 3) Ready -> transparent relay.
    if ws.send(text(r#"{"t":"ready"}"#)).await.is_err() {
        return;
    }
    relay(ws, shell).await;
}

async fn relay(mut ws: WebSocket, mut shell: ssh::Shell) {
    loop {
        tokio::select! {
            msg = shell.channel.wait() => match msg {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    if ws.send(Message::Binary(data)).await.is_err() {
                        break;
                    }
                }
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                _ => {} // window adjusts, exit status, etc. — ignore; Eof/Close ends it
            },
            ws_msg = ws.recv() => match ws_msg {
                Some(Ok(Message::Text(t))) => {
                    if !on_text(&shell.channel, &t).await {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                _ => {} // binary/ping/pong from the client — ignore
            },
        }
    }
    let _ = shell.channel.eof().await;
}

/// Handle one client control frame during the relay. Returns false to end the session.
async fn on_text(channel: &Channel<Msg>, text: &str) -> bool {
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return true, // ignore malformed frames
    };
    match v.get("t").and_then(|t| t.as_str()) {
        Some("in") => {
            if let Some(d) = v.get("d").and_then(|d| d.as_str()) {
                if channel.data_bytes(d.as_bytes().to_vec()).await.is_err() {
                    return false;
                }
            }
        }
        Some("sz") => {
            let cols = v.get("cols").and_then(|c| c.as_u64()).unwrap_or(80) as u32;
            let rows = v.get("rows").and_then(|r| r.as_u64()).unwrap_or(24) as u32;
            let _ = channel.window_change(cols, rows, 0, 0).await;
        }
        _ => {}
    }
    true
}

/// Read frames until the opening `connect` arrives (the frontend sends it first).
async fn recv_connect(ws: &mut WebSocket) -> Option<Connect> {
    while let Some(Ok(msg)) = ws.recv().await {
        if let Message::Text(t) = msg {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                if v.get("t").and_then(|t| t.as_str()) == Some("connect") {
                    return serde_json::from_value(v).ok();
                }
            }
        }
    }
    None
}

async fn err(ws: &mut WebSocket, reason: &str) {
    let payload = serde_json::json!({ "t": "err", "reason": reason }).to_string();
    let _ = ws.send(Message::Text(payload.into())).await;
    let _ = ws.send(Message::Close(None)).await;
}

fn text(s: &str) -> Message {
    Message::Text(s.to_string().into())
}
