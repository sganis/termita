# Termita — Specification

> tiny browser terminal for hard-to-reach hosts

## Goal

A browser-based terminal that connects to a **remote host over SSH**. The user
enters a host, username, and password in the browser; the server opens an SSH
session natively and streams the **remote** shell back to the browser.

Termita is **only an SSH client** — it has no shell, no user accounts, and no
`sshd` of its own. Authentication is delegated entirely to the remote host's SSH
login. The container holds no user data; each connection is just an SSH session.

```text
Browser (Svelte 5 + xterm.js) ──WebSocket──▶ termita (Rust: axum + russh) ──▶ remote host shell
```

## Components

| Layer | Choice |
|---|---|
| Frontend | Svelte 5 + [xterm.js](https://xtermjs.org) (+ fit addon), bundled by **Vite via Bun** → `web/dist` |
| Backend | A single **Rust** binary: [`axum`](https://github.com/tokio-rs/axum) serves the bundle + a `/ws` WebSocket |
| SSH | [`russh`](https://github.com/Eugeny/russh) — a pure-Rust SSH client (crypto backend: `ring`) |
| Bundle | `web/dist` is embedded into the binary at compile time (`rust-embed`) |
| Image | static `x86_64-unknown-linux-musl` binary on `FROM scratch`; runs as a non-root, arbitrary UID |

### Why native SSH (russh) replaced the spawned `ssh` client

The original implementation spawned the OpenSSH `ssh` client inside a PTY
(`node-pty`, under Node) and inferred state by scraping the client's output. russh
speaks the SSH protocol directly, eliminating the indirection and its problems:

- **Deterministic auth** — success/failure is the protocol's answer, not a guess
  from timing or a printed sentinel.
- **Real PTY channel** — a `pty-req` + `shell` request runs the user's login shell
  directly. No `/bin/sh -c` bootstrap, so csh/tcsh/zsh/fish work without special
  handling.
- **No native module** — `node-pty` had no Linux prebuilt and compiled from source
  via node-gyp; russh's PTY is part of the protocol, so the build needs no C
  toolchain for it.
- **No `getpwuid`** — the OpenSSH client called `getpwuid()` and failed under
  OpenShift's random UID (*"No user exists for uid …"*). russh never makes that
  call, so the `/etc/passwd` workaround is gone and any UID works.

## Connection flow

1. The browser loads the connect form: **host / username / password**, with the
   SSH **port** under an *Advanced* toggle (default `22`). The last
   host/username/port are persisted in `localStorage` and pre-filled on the next
   visit; the password is never stored.
2. On submit it opens `/ws` and (on open) sends a `connect` message with the
   credentials and the initial terminal size.
3. The server (`ssh.rs`):
   - opens a TCP+SSH connection with a 15s timeout; the server's host key is
     accepted (trust-on-first-use);
   - authenticates with the password, falling back to **keyboard-interactive**
     (answering each prompt with the password) for PAM-style password auth;
   - opens a session channel, requests a PTY (`xterm-256color`, the client's
     cols/rows), and requests a login shell.
4. **Establishment is deterministic:**
   - **All requests succeed** → server sends `{t:"ready"}` and relays the shell.
     Whatever the login shell prints (MOTD/banner) is shown as normal terminal
     output.
   - **Any step fails** → server sends `{t:"err", reason}` and closes; the client
     stays on the login form and shows the reason (auth failure, unresolved host,
     connection refused/timeout, or a shell-setup error).
5. After `ready` it is a transparent relay: keystrokes → SSH channel, remote output
   → browser (binary frames), plus terminal resize (forwarded as `window-change`).
   When the channel closes (logout, dropped connection) the socket closes and the
   UI offers **New connection**.

**Nothing is stored**: the password is used once to authenticate, then discarded.

## Wire protocol

Client → server (JSON text frames):

- `{"t":"connect","host":…,"user":…,"port":22,"password":…,"cols":N,"rows":M}` — open the session
- `{"t":"in","d":"<keystrokes>"}` — input
- `{"t":"sz","cols":N,"rows":M}` — resize

Server → client:

- **Control** (JSON text frames): `{"t":"ready"}` | `{"t":"err","reason":"…"}`
- **Output** (binary frames): raw remote-shell bytes

The client distinguishes the two by frame type (text = control, binary = output)
and buffers any output that arrives before the terminal element mounts. The wire
protocol is **identical** to the previous Node implementation, so the frontend was
not modified during the rewrite.

## Configuration (environment)

| Var | Default | Purpose |
|---|---|---|
| `PORT` | `3000` | listen port |
| `HOST` | `0.0.0.0` | bind address inside the container |
| `ALLOWED_HOSTS` | _(empty = any)_ | comma-separated allowlist of SSH targets |

## Security model

- **Client only** — there is no `sshd`. You connect *out*, never *into* the container.
- **Password transport** — the password travels browser → server over the
  WebSocket. Run behind **TLS (https/wss)** in any real deployment.
- **Bind to localhost** (`-p 127.0.0.1:3000:3000`). The relay can reach any host the
  container can reach, so do not expose it unauthenticated on a network. Use
  `ALLOWED_HOSTS` to restrict targets.
- **No credential storage** — the password authenticates the SSH session and is then
  dropped; it is never written to disk, logs, or process arguments. The browser
  remembers **host/username/port** in `localStorage`, but **never the password**.
- **Host keys** — trust-on-first-use: the server accepts the remote's key and keeps
  no persistent `known_hosts`. Equivalent to OpenSSH's
  `StrictHostKeyChecking=accept-new`. (A future enhancement could pin/verify keys.)

## Build & run

```bash
docker build -t termita .
docker run --rm -it -p 127.0.0.1:3000:3000 termita
# open http://localhost:3000 → enter host / username / password → Connect
```

Three-stage build: **bun** builds the frontend → **cargo** builds a static musl
binary that embeds it → `FROM scratch` ships only the binary. The build needs
network only for bun + cargo dependency downloads; run it where you have internet
(locally / CI) and push the image, or vendor dependencies for an offline in-cluster
build.

Local development:

```bash
cargo run                 # Rust relay on :3000 (serves web/dist)
cd web && bun run dev     # Vite dev server (proxies /ws → :3000)
```

## File layout

```text
termita/
├─ Cargo.toml           Rust backend manifest
├─ src/
│  ├─ main.rs           config (PORT/HOST/ALLOWED_HOSTS) + startup
│  ├─ serve.rs          axum router, embedded static assets, /ws upgrade
│  ├─ bridge.rs         one WebSocket ↔ one SSH shell (protocol + relay)
│  └─ ssh.rs            russh client: connect, auth, pty, shell
├─ web/
│  ├─ index.html
│  ├─ src/{main.js, app.svelte}   connect form + xterm.js terminal
│  ├─ vite.config.js / svelte.config.js / package.json
│  └─ dist/             build output (gitignored), embedded into the binary
├─ Dockerfile          bun build → static musl cargo build → scratch
└─ doc/spec.md         this file
```

## Acceptance criteria

The frontend, the HTTP surface (static serving, SPA fallback, asset caching), the
`/ws` upgrade, the `connect`-frame parsing, host/username validation, the
`ALLOWED_HOSTS` allowlist, and connect-error mapping (DNS / refused / timeout) are
verified. The items below need a live SSH host and should be re-checked before
release:

- ✓ Image builds; container starts and serves `http://localhost:3000`.
- ✓ Unreachable / unresolvable host → stays on the form with a clear reason.
- ✓ Missing host/username → stays on the form ("Host and username are required").
- ☐ Correct credentials → interactive **remote** shell (resize, Ctrl+C, arrow keys,
  tab completion, `vim`, `top` all work — provided by the remote host's PTY).
- ☐ Wrong password → client **stays on the login form** with
  *"Authentication failed — check your username and password."* (no terminal flash).
- ☐ A remote account whose default login shell is `csh`/`tcsh`/`zsh`/`fish` connects
  normally (no bootstrap hack — russh requests a plain login shell).
- ☐ Container runs under an **arbitrary UID** (OpenShift default) — no
  *"No user exists for uid …"*.
- ☐ The connect form pre-fills the last host / username / port and never stores the
  password; the SSH port hides under *Advanced* (default `22`).

## Known limitations

- Only single password / keyboard-interactive password auth is handled — no
  multi-step 2FA, and no SSH key / certificate auth yet.
- Host keys are trusted on first use and not persisted; there is no known_hosts
  pinning.

## Out of scope / superseded approaches

Earlier iterations (a shell *inside* the container; Keycloak/OIDC auth with
server-side sessions and per-user PVCs; per-user isolation) were deliberately
dropped: authenticating to the remote host *is* the authentication, and the remote
host's own OS enforces identity and isolation. Re-introducing identity-aware access
(an allowlist tied to an IdP, or SSH-key/CA provisioning) would be a future,
separately-scoped effort.
