// src/ssh.rs
// Native SSH client (russh): connect, authenticate with a password, request a PTY
// and a login shell. No external `ssh` binary and no PTY subprocess — russh speaks
// the SSH protocol directly, so we get a real auth result and a real shell channel.
// (This is also why the OpenShift random-UID problem disappears: nothing here calls
// getpwuid the way the OpenSSH client did.)
//
// Optionally the target is reached through a jump (bastion) host: we connect and
// authenticate to the bastion, open a direct-tcpip channel to the target, and run
// the target SSH session *over that channel* (russh's `connect_stream`). The same
// password authenticates both hops.
use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, Config, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::ssh_key::PublicKey;
use russh::Channel;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A jump (bastion) host the target is reached through.
pub struct Jump {
    pub host: String,
    pub port: u16,
    pub user: String,
}

pub struct Params {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub cols: u32,
    pub rows: u32,
    /// When set, tunnel the target session through this bastion.
    pub jump: Option<Jump>,
}

/// A live shell session. `_handle` owns the target connection (dropping it closes
/// the session); `_jump` (when tunneled) owns the bastion connection so the
/// direct-tcpip tunnel stays open; `channel` carries the remote shell's bytes.
pub struct Shell {
    _handle: Handle<Client>,
    _jump: Option<Handle<Client>>,
    pub channel: Channel<client::Msg>,
}

/// Trust-on-first-use host keys, matching the old `StrictHostKeyChecking=accept-new`.
/// termita is an ephemeral relay with no persistent known_hosts; see the README's
/// security note. The same handler is used for the bastion and the target.
struct Client;

impl client::Handler for Client {
    type Error = russh::Error;
    async fn check_server_key(&mut self, _key: &PublicKey) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

fn config() -> Arc<Config> {
    Arc::new(Config {
        inactivity_timeout: None, // don't garbage-collect an idle interactive shell
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        nodelay: true, // low-latency keystrokes
        ..Default::default()
    })
}

/// Connect, authenticate, and start a login shell. With `p.jump` set, the target is
/// reached through the bastion via a direct-tcpip tunnel; the same password
/// authenticates both hops. On any failure, returns a concise human-readable reason.
pub async fn connect(p: &Params) -> Result<Shell, String> {
    let cfg = config();

    let (mut handle, jump) = match &p.jump {
        None => (dial(cfg.clone(), &p.host, p.port).await?, None),
        Some(j) => {
            let mut jh = dial_jump(cfg.clone(), j).await?;
            if !authenticate(&mut jh, &j.user, &p.password).await? {
                return Err("Jump host authentication failed — check the username and password.".into());
            }
            // Open a tunnel to the target through the bastion, then run the target
            // SSH session over it. The Channel's stream owns a cloned session sender,
            // so moving `jh` into the Shell afterwards keeps the tunnel alive.
            let stream = jh
                .channel_open_direct_tcpip(p.host.as_str(), p.port as u32, "127.0.0.1", 0)
                .await
                .map_err(|_| "Jump host could not open a tunnel to the target.".to_string())?
                .into_stream();
            let h = client::connect_stream(cfg.clone(), stream, Client)
                .await
                .map_err(|e| format!("Could not start SSH over the jump tunnel: {e}"))?;
            (h, Some(jh))
        }
    };

    if !authenticate(&mut handle, &p.user, &p.password).await? {
        return Err("Authentication failed — check your username and password.".into());
    }

    let channel = open_shell(&handle, p.cols, p.rows).await?;
    Ok(Shell { _handle: handle, _jump: jump, channel })
}

/// Dial a host directly with a connect timeout, mapping failures to a short reason.
async fn dial(cfg: Arc<Config>, host: &str, port: u16) -> Result<Handle<Client>, String> {
    let connecting = client::connect(cfg, (host, port), Client);
    match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
        Err(_) => Err("Could not connect to the host (timed out).".into()),
        Ok(Err(e)) => Err(connect_reason(host, &e)),
        Ok(Ok(h)) => Ok(h),
    }
}

/// Dial the bastion, with jump-flavored error wording.
async fn dial_jump(cfg: Arc<Config>, j: &Jump) -> Result<Handle<Client>, String> {
    let connecting = client::connect(cfg, (j.host.as_str(), j.port), Client);
    match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
        Err(_) => Err("Could not reach the jump host (timed out).".into()),
        Ok(Err(e)) => Err(format!("Jump host: {}", connect_reason(&j.host, &e))),
        Ok(Ok(h)) => Ok(h),
    }
}

/// Request a PTY and a login shell on an authenticated connection.
async fn open_shell(handle: &Handle<Client>, cols: u32, rows: u32) -> Result<Channel<client::Msg>, String> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|_| "Logged in, but the server refused to open a shell channel.".to_string())?;
    channel
        .request_pty(false, "xterm-256color", cols, rows, 0, 0, &[])
        .await
        .map_err(|_| "Logged in, but the remote PTY request failed.".to_string())?;
    channel
        .request_shell(true)
        .await
        .map_err(|_| "Logged in, but the remote shell did not start.".to_string())?;
    Ok(channel)
}

/// Password auth first; fall back to keyboard-interactive answering every prompt
/// with the password — that's how many PAM-backed servers actually do "passwords".
async fn authenticate(handle: &mut Handle<Client>, user: &str, password: &str) -> Result<bool, String> {
    let auth_err = |e: russh::Error| format!("Authentication error: {e}");

    let by_password = handle
        .authenticate_password(user.to_string(), password.to_string())
        .await
        .map_err(auth_err)?;
    if by_password.success() {
        return Ok(true);
    }

    let mut res = handle
        .authenticate_keyboard_interactive_start(user.to_string(), None)
        .await
        .map_err(auth_err)?;
    loop {
        match res {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| password.to_string()).collect();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    fn io(e: Error) -> russh::Error {
        russh::Error::IO(e)
    }

    #[test]
    fn refused_and_timeout_map_to_clean_reasons() {
        assert_eq!(connect_reason("h", &io(ErrorKind::ConnectionRefused.into())), "Connection refused.");
        assert_eq!(connect_reason("h", &io(ErrorKind::TimedOut.into())), "Connection timed out.");
    }

    #[test]
    fn dns_failures_are_prettified_on_both_platforms() {
        // Linux phrasing
        let linux = io(Error::new(ErrorKind::Other, "failed to lookup address information: Name or service not known"));
        assert_eq!(connect_reason("badhost", &linux), "Could not resolve hostname badhost.");
        // Windows phrasing
        let win = io(Error::new(ErrorKind::Other, "No such host is known. (os error 11001)"));
        assert_eq!(connect_reason("badhost", &win), "Could not resolve hostname badhost.");
    }

    #[test]
    fn unknown_io_errors_fall_through_to_generic() {
        let other = io(Error::new(ErrorKind::Other, "boom"));
        assert_eq!(connect_reason("h", &other), "Could not connect: boom");
    }
}
