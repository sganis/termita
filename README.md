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

1. The browser loads the connect form (host / username / password; the SSH port and
   an optional **jump host** — `user@jumper` — are under **Advanced**). The last
   host/username/port/jump are remembered in `localStorage`; the password is not.
2. On submit it opens `/ws` and sends `{t:"connect", host, user, port, password, jump, cols, rows}`.
3. The server opens an SSH session with russh, authenticates with the password
   (trying keyboard-interactive as a fallback), then requests a PTY and a login
   shell. On success it sends `{t:"ready"}`; on failure `{t:"err", reason}`. When a
   jump host is given it first connects and authenticates to the bastion (same
   password), opens a direct-tcpip tunnel to the target, and runs the target session
   over it.
4. After `ready` it's a transparent relay: keystrokes → SSH channel, remote output →
   browser (binary frames), plus terminal resize. When the session ends the socket
   closes and the UI shows a "New connection" button.

Nothing is stored: the password is used once to authenticate and discarded.

## Build & run

**Prerequisites:** a [Rust toolchain](https://rustup.rs) and [Bun](https://bun.sh).
You do **not** need Docker to run termita locally — Docker is only for building the
deployable container image.

### Run locally (cargo)

Build the frontend once, then build and run the server:

```bash
cd web && bun install && bun run build && cd ..   # → web/dist
cargo run                                         # builds + runs on http://localhost:3000
```

`cargo run` compiles the binary (with the frontend embedded) and starts the server;
open http://localhost:3000 and enter host / username / password. Configure with
environment variables (see [Configuration](#configuration)):

```bash
PORT=8080 ALLOWED_HOSTS=10.0.0.5 cargo run
```

For a standalone, optimized artifact, `cargo build --release` produces **one
self-contained binary** at `target/release/termita` (`termita.exe` on Windows) —
copy it anywhere and run it; it needs no files beside it.

### Build the container image (for deployment)

```bash
docker build -t termita .
docker run --rm -it -p 127.0.0.1:3000:3000 termita
```

The image is a static binary on `FROM scratch` and is fully self-contained; the
build only needs network for bun + cargo downloads. See [Deployment](#deployment)
for pushing it to a registry / OpenShift.

## Deployment

See **[`doc/deploy.md`](doc/deploy.md)** for full instructions — Docker/Podman and
**OpenShift** (build off-cluster → push → run), with a ready-to-apply manifest at
[`deploy/openshift.yaml`](deploy/openshift.yaml). In short: termita runs under any
UID under the default `restricted-v2` SCC (no `anyuid`, no `/etc/passwd` hack), so
deploying is just running one ~3.4 MB container behind an edge-TLS Route.

## Local development

For frontend work with hot-reload, run the backend and the Vite dev server side by
side — Vite serves the UI (HMR, open the URL it prints) and proxies `/ws` to the
backend:

```bash
cargo run                 # terminal 1: Rust relay on :3000 (handles /ws)
cd web && bun run dev     # terminal 2: Vite dev server with HMR → proxies /ws → :3000
```

Unlike *Run locally* above, edits in `web/src` appear instantly without a rebuild.
(`cargo run` still needs `web/dist` to exist to compile, so run `cd web && bun run
build` once; in debug builds `rust-embed` reads it from disk.)

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

- `{"t":"connect","host":…,"user":…,"port":22,"password":…,"jump":"user@jumper","cols":N,"rows":M}` — open the session (`jump` optional; empty/absent = direct)
- `{"t":"in","d":"<keystrokes>"}` — input
- `{"t":"sz","cols":N,"rows":M}` — resize

Server → client: `{"t":"ready"}` / `{"t":"err","reason":…}` as text; raw remote-shell
bytes as binary frames.
