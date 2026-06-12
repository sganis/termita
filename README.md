# Termita

> tiny browser terminal for hard-to-reach hosts

A browser terminal that SSHes into a remote host. You type a host, username, and
password in the browser; the server opens an SSH session **natively** (pure-Rust
[russh](https://github.com/Eugeny/russh) — no `ssh` subprocess) and streams the
**remote** shell back to [xterm.js](https://xtermjs.org).

```text
Browser (Svelte 5 + xterm.js) ──WebSocket──▶ termita (Rust: axum + russh) ──▶ remote host shell
```

Termita is **only an SSH client** — it has no shell, accounts, or `sshd` of its own.
Authentication is the remote host's own SSH login.

## Stack

- **Frontend:** Svelte 5 + xterm.js (fit addon), bundled by **Vite via Bun** → `web/dist`.
- **Backend:** a single **Rust** binary — [`axum`](https://github.com/tokio-rs/axum)
  serves the bundle and a `/ws` WebSocket; [`russh`](https://github.com/Eugeny/russh)
  speaks SSH directly.
- **Bundle:** the frontend is embedded into the binary at compile time
  (`rust-embed`), so the runtime artifact is **just one file**.
- **Image:** a static `x86_64-unknown-linux-musl` binary on `FROM scratch` — no
  Node, no Bun, no `node_modules`, no `ssh` client.

### Why native SSH (russh) instead of spawning `ssh`

The previous version spawned the OpenSSH `ssh` client inside a PTY (`node-pty`) and
scraped its output to detect the password prompt and a readiness "sentinel". russh
speaks the SSH protocol directly, which removes a whole class of problems:

- **Real auth result** — success/failure comes from the protocol, not from guessing.
- **Real PTY channel** — no `/bin/sh -c` bootstrap, so csh/tcsh/fish login shells
  "just work".
- **No native module** — `node-pty` had no Linux prebuilt and compiled from source;
  russh's PTY is part of the protocol.
- **No `getpwuid`** — the OpenSSH client failed under OpenShift's random UID with
  *"No user exists for uid …"*; russh never makes that call, so any UID is fine.

## How it works

1. The browser loads the connect form (host / username / password; the SSH port is
   under **Advanced**). The last host/username/port are remembered in `localStorage`;
   the password is not.
2. On submit it opens `/ws` and sends `{t:"connect", host, user, port, password, cols, rows}`.
3. The server opens an SSH session with russh, authenticates with the password
   (trying keyboard-interactive as a fallback), then requests a PTY and a login
   shell. On success it sends `{t:"ready"}`; on failure `{t:"err", reason}`.
4. After `ready` it's a transparent relay: keystrokes → SSH channel, remote output →
   browser (binary frames), plus terminal resize. When the session ends the socket
   closes and the UI shows a "New connection" button.

Nothing is stored: the password is used once to authenticate and discarded.

## Build & run

```bash
docker build -t termita .
docker run --rm -it -p 127.0.0.1:3000:3000 termita
# open http://localhost:3000 → enter host / username / password → Connect
```

The build is self-contained: bun builds the frontend, cargo builds the static
binary that embeds it, and the runtime stage is `FROM scratch`. The only thing the
**build** needs network for is bun + cargo dependency downloads; run `docker build`
where you have internet (e.g. locally or in CI) and push the image to OpenShift's
registry, or vendor dependencies for an offline in-cluster build.

## Local development

```bash
cargo run                 # terminal 1: Rust relay on :3000 (serves web/dist)
cd web && bun run dev     # terminal 2: Vite dev server (proxies /ws → :3000)
```

Build the frontend once (`cd web && bun run build`) so the backend has a `web/dist`
to serve/embed. In debug builds `rust-embed` reads `web/dist` from disk, so a
frontend rebuild is picked up without recompiling the backend.

## Folder structure

```text
termita/
├─ Cargo.toml            Rust backend manifest
├─ src/                  Rust backend
│  ├─ main.rs            config (PORT/HOST/ALLOWED_HOSTS) + startup
│  ├─ serve.rs           axum router, embedded static assets, /ws
│  ├─ bridge.rs          one WebSocket <-> one SSH shell (wire protocol + relay)
│  └─ ssh.rs             russh client: connect, auth, pty, shell
├─ web/                  frontend (Svelte 5 + xterm.js)
│  ├─ index.html
│  ├─ src/{main.js, app.svelte}
│  ├─ vite.config.js / svelte.config.js / package.json
│  └─ dist/              build output (gitignored, embedded into the binary)
├─ Dockerfile           bun build → static musl cargo build → scratch
└─ doc/spec.md          specification
```

## Configuration

| Var | Default | Purpose |
|---|---|---|
| `PORT` | `3000` | listen port |
| `HOST` | `0.0.0.0` | bind address inside the container |
| `ALLOWED_HOSTS` | _(empty = any)_ | comma-separated allowlist of SSH targets |

## Security

- **Bind to localhost** (`-p 127.0.0.1:3000:3000`). The relay can reach any host the
  container can reach, so don't expose it unauthenticated on a network.
- **Use TLS (https/wss)** in any real deployment — the password travels from the
  browser to the server over the WebSocket.
- Set **`ALLOWED_HOSTS`** to restrict which hosts may be reached.
- **Host keys:** trust-on-first-use (the server accepts the remote's key; there is no
  persistent `known_hosts`). Equivalent to OpenSSH's `StrictHostKeyChecking=accept-new`.
- **No credential storage:** the password authenticates the SSH session and is then
  dropped; it is never written to disk, logs, or process arguments. The browser
  remembers host/username/port for convenience, never the password.

## Protocol

Client → server (JSON text frames):

- `{"t":"connect","host":…,"user":…,"port":22,"password":…,"cols":N,"rows":M}` — open the session
- `{"t":"in","d":"<keystrokes>"}` — input
- `{"t":"sz","cols":N,"rows":M}` — resize

Server → client: `{"t":"ready"}` / `{"t":"err","reason":…}` as text; raw remote-shell
bytes as binary frames.
