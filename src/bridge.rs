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
    /// Optional jump (bastion) host as `user@host[:port]`; empty = no jump.
    #[serde(default)]
    jump: String,
    #[serde(default = "default_cols")]
    cols: u32,
    #[serde(default = "default_rows")]
    rows: u32,
}
fn default_port() -> u16 { 22 }
fn default_cols() -> u32 { 80 }
fn default_rows() -> u32 { 24 }

/// An empty allowlist permits any host; otherwise the host must be listed exactly.
fn host_allowed(allowed: &[String], host: &str) -> bool {
    allowed.is_empty() || allowed.iter().any(|h| h == host)
}

/// Parse a jump-host string `user@host[:port]` into an `ssh::Jump`. The user is
/// optional (defaults to the target's username); a missing or malformed port
/// falls back to 22 so the jump is never silently dropped. Empty input → no jump.
fn parse_jump(s: &str, default_user: &str) -> Option<ssh::Jump> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (user, hostport) = match s.split_once('@') {
        Some((u, hp)) => (u.trim().to_string(), hp.trim()),
        None => (default_user.to_string(), s),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(port) => (h.to_string(), port),
            Err(_) => (hostport.to_string(), 22),
        },
        None => (hostport.to_string(), 22),
    };
    if host.is_empty() {
        return None;
    }
    Some(ssh::Jump { host, port, user })
}

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
    if !host_allowed(allowed, &host) {
        return err(&mut ws, &format!("Host not allowed: {host}")).await;
    }

    // Optional jump host (the bastion is dialed directly by the relay, so it is
    // subject to the same allowlist as the target).
    let jump = parse_jump(&conn.jump, &user);
    if let Some(j) = &jump {
        if !host_allowed(allowed, &j.host) {
            return err(&mut ws, &format!("Jump host not allowed: {}", j.host)).await;
        }
        let _ = ws.send(text(r#"{"t":"status","phase":"jump"}"#)).await;
    }

    // 2) Connect + authenticate (russh bundles transport, auth, and shell setup).
    let params = ssh::Params {
        host,
        port: conn.port,
        user,
        password: conn.password,
        cols: conn.cols,
        rows: conn.rows,
        jump,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn allowlist_empty_allows_any() {
        assert!(host_allowed(&[], "anything"));
    }

    #[test]
    fn allowlist_restricts_to_listed_hosts() {
        let allowed = vec!["a.example".to_string(), "b.example".to_string()];
        assert!(host_allowed(&allowed, "b.example"));
        assert!(!host_allowed(&allowed, "c.example"));
    }

    #[test]
    fn connect_applies_defaults_when_omitted() {
        let c: Connect = serde_json::from_value(json!({ "host": "h", "user": "u" })).unwrap();
        assert_eq!((c.host.as_str(), c.user.as_str(), c.password.as_str()), ("h", "u", ""));
        assert_eq!((c.port, c.cols, c.rows), (22, 80, 24));
    }

    #[test]
    fn connect_parses_explicit_values() {
        let c: Connect = serde_json::from_value(json!({
            "host": "h", "user": "u", "port": 2222, "password": "p", "cols": 120, "rows": 40
        }))
        .unwrap();
        assert_eq!((c.port, c.cols, c.rows, c.password.as_str()), (2222, 120, 40, "p"));
    }

    #[test]
    fn connect_jump_defaults_empty_and_parses_when_present() {
        let c: Connect = serde_json::from_value(json!({ "host": "h", "user": "u" })).unwrap();
        assert_eq!(c.jump.as_str(), "");
        let c: Connect =
            serde_json::from_value(json!({ "host": "h", "user": "u", "jump": "a@b" })).unwrap();
        assert_eq!(c.jump.as_str(), "a@b");
    }

    #[test]
    fn parse_jump_empty_is_none() {
        assert!(parse_jump("", "me").is_none());
        assert!(parse_jump("   ", "me").is_none());
    }

    #[test]
    fn parse_jump_host_only_defaults_user_and_port() {
        let j = parse_jump("bastion", "me").unwrap();
        assert_eq!((j.host.as_str(), j.port, j.user.as_str()), ("bastion", 22, "me"));
    }

    #[test]
    fn parse_jump_user_at_host() {
        let j = parse_jump("admin@bastion", "me").unwrap();
        assert_eq!((j.host.as_str(), j.port, j.user.as_str()), ("bastion", 22, "admin"));
    }

    #[test]
    fn parse_jump_user_at_host_port() {
        let j = parse_jump("admin@bastion:2222", "me").unwrap();
        assert_eq!((j.host.as_str(), j.port, j.user.as_str()), ("bastion", 2222, "admin"));
    }

    #[test]
    fn parse_jump_bad_port_falls_back_to_22() {
        let j = parse_jump("admin@bastion:nope", "me").unwrap();
        assert_eq!((j.host.as_str(), j.port), ("bastion:nope", 22));
    }
}
