// src/ssh.rs
// Native SSH client (russh): connect, authenticate with a password, request a PTY
// and a login shell. No external `ssh` binary and no PTY subprocess — russh speaks
// the SSH protocol directly, so we get a real auth result and a real shell channel.
// (This is also why the OpenShift random-UID problem disappears: nothing here calls
// getpwuid the way the OpenSSH client did.)
use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, Config, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::ssh_key::PublicKey;
use russh::Channel;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Params {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub cols: u32,
    pub rows: u32,
}

/// A live shell session. `_handle` owns the connection (dropping it closes the
/// session); `channel` carries the remote shell's bytes for the relay.
pub struct Shell {
    _handle: Handle<Client>,
    pub channel: Channel<client::Msg>,
}

/// Trust-on-first-use host keys, matching the old `StrictHostKeyChecking=accept-new`.
/// termita is an ephemeral relay with no persistent known_hosts; see the README's
/// security note.
struct Client;

impl client::Handler for Client {
    type Error = russh::Error;
    async fn check_server_key(&mut self, _key: &PublicKey) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// Connect, authenticate, and start a login shell. On any failure, returns a
/// concise human-readable reason suitable for showing on the login form.
pub async fn connect(p: &Params) -> Result<Shell, String> {
    let config = Arc::new(Config {
        inactivity_timeout: None, // don't garbage-collect an idle interactive shell
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        nodelay: true, // low-latency keystrokes
        ..Default::default()
    });

    let connecting = client::connect(config, (p.host.as_str(), p.port), Client);
    let mut handle = match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
        Err(_) => return Err("Could not connect to the host (timed out).".into()),
        Ok(Err(e)) => return Err(connect_reason(&p.host, &e)),
        Ok(Ok(h)) => h,
    };

    if !authenticate(&mut handle, p).await? {
        return Err("Authentication failed — check your username and password.".into());
    }

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|_| "Logged in, but the server refused to open a shell channel.".to_string())?;
    channel
        .request_pty(false, "xterm-256color", p.cols, p.rows, 0, 0, &[])
        .await
        .map_err(|_| "Logged in, but the remote PTY request failed.".to_string())?;
    channel
        .request_shell(true)
        .await
        .map_err(|_| "Logged in, but the remote shell did not start.".to_string())?;

    Ok(Shell { _handle: handle, channel })
}

/// Password auth first; fall back to keyboard-interactive answering every prompt
/// with the password — that's how many PAM-backed servers actually do "passwords".
async fn authenticate(handle: &mut Handle<Client>, p: &Params) -> Result<bool, String> {
    let auth_err = |e: russh::Error| format!("Authentication error: {e}");

    let by_password = handle
        .authenticate_password(p.user.clone(), p.password.clone())
        .await
        .map_err(auth_err)?;
    if by_password.success() {
        return Ok(true);
    }

    let mut res = handle
        .authenticate_keyboard_interactive_start(p.user.clone(), None)
        .await
        .map_err(auth_err)?;
    loop {
        match res {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| p.password.clone()).collect();
                res = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(auth_err)?;
            }
        }
    }
}

/// Map a russh connect error to a short, user-facing reason.
fn connect_reason(host: &str, e: &russh::Error) -> String {
    use std::io::ErrorKind;
    if let russh::Error::IO(io) = e {
        return match io.kind() {
            ErrorKind::ConnectionRefused => "Connection refused.".into(),
            ErrorKind::TimedOut => "Connection timed out.".into(),
            ErrorKind::HostUnreachable | ErrorKind::NetworkUnreachable => "No route to host.".into(),
            _ => {
                // DNS resolution failures surface here as a generic io error, with
                // wording that differs by platform (Linux: "...not known" /
                // "failed to lookup address"; Windows: "No such host is known").
                let s = io.to_string();
                if s.contains("lookup address")
                    || s.contains("not known")
                    || s.contains("No such host")
                    || s.contains("resolve")
                {
                    format!("Could not resolve hostname {host}.")
                } else {
                    format!("Could not connect: {s}")
                }
            }
        };
    }
    format!("Could not connect: {e}")
}
