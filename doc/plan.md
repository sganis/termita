# Termita — Rust/russh Rewrite Plan

> Rewrite the backend from Node (`node-pty` + `ssh` subprocess) to **Rust + russh**
> (native, pure-Rust SSH client). Keep the Svelte 5 + xterm.js frontend unchanged.
> Ship a single static binary built in a multi-stage Docker build.

## Why (the real win is deployment, not perf)

Termita is a byte relay — it's I/O-bound and the `ssh` processes dominate under
load, so Rust is **not** a perf/scale upgrade. The win is operational:

| Current pain (Node) | After (Rust + russh) |
|---|---|
| `node-pty` has no Linux prebuilt → compiles from source via node-gyp | PTY is part of the SSH protocol via `russh` — no native module, no node-gyp |
| Must ship `node_modules`; Windows binary ≠ Linux binary | Single static binary (`x86_64-unknown-linux-musl`) — copy one file |
| Bun installer `curl`'d in the image; `bun install` needs internet | `cargo build` (crates vendorable for offline OCP builds); no Bun in the image |
| Image needs `openssh-clients` + `bash` | **No `ssh` subprocess** → no openssh-clients; russh speaks SSH directly |
| Sentinel scrape + password injection + csh/tcsh `/bin/sh -c` hack | Real protocol auth result + real PTY channel — **all hacks deleted** |
| OpenShift random UID → `ssh` `getpwuid()` fails: "No user exists for uid" | **Bug gone** — russh never calls `getpwuid`; runs under any UID |
| ~450 MB image | Tiny image (`FROM scratch` + one binary, frontend embedded) |

The Svelte + xterm.js frontend talks plain WebSocket JSON and doesn't care what's
on the other end — it is **not modified**.

## New folder structure

Separate the Rust backend (root `src/` + `Cargo.toml`) from the web frontend
(`web/`). Names follow the project's standards: lowercase, singular, short, no
underscores.

```text
termita/
├─ Cargo.toml              # Rust backend manifest
├─ Cargo.lock
├─ src/                    # Rust backend (the old Svelte source moves to web/)
│  ├─ main.rs              # config (PORT/HOST/ALLOWED_HOSTS) + startup   (~80)
│  ├─ serve.rs             # axum router, embedded static assets, /ws     (~120)
│  ├─ bridge.rs            # per-connection: parse frames, drive ssh, relay (~150)
│  └─ ssh.rs               # russh client: connect, auth, pty, shell      (~150)
├─ web/                    # frontend, moved out of the repo root
│  ├─ index.html
│  ├─ package.json         # frontend-only now (svelte, vite, xterm)
│  ├─ bun.lock
│  ├─ vite.config.js       # outDir: dist
│  ├─ svelte.config.js
│  ├─ src/{main.js, app.svelte}
│  └─ dist/                # build output (gitignored) → embedded into the binary
├─ Dockerfile             # 3 stages: web build → rust build → scratch runtime
├─ doc/spec.md            # updated to the Rust architecture
└─ README.md              # updated
```

Each Rust file stays in the 80–150 line range (well under the 600-line limit).

## Behavior mapping (Node → russh)

| Concern | Now (Node) | After (Rust / russh) |
|---|---|---|
| Transport | spawn `ssh -tt` in node-pty | `russh::client::connect` (native TCP+SSH) |
| Auth | type password at PTY prompt; infer success from a sentinel | `authenticate_password` (+ keyboard-interactive fallback) — real pass/fail |
| Host key | `StrictHostKeyChecking=accept-new` | `Handler::check_server_key` accepts (TOFU parity; documented) |
| PTY + shell | remote `/bin/sh -c 'printf sentinel; exec $SHELL -l'` | `channel.request_pty("xterm-256color", cols, rows)` + `request_shell()` |
| Ready signal | scrape `TERMITA_READY_<nonce>` | shell request ack → `{t:"ready"}` |
| Banner/MOTD | hidden until sentinel | shown normally (real terminal behavior — minor UX change) |
| csh/tcsh/fish | `/bin/sh -c` bootstrap wrapper | not needed — `request_shell` runs the login shell directly |
| Relay | `onData → ws`; `ws in → pty` | `tokio::select!` between channel data and ws frames |
| Resize | `term.resize` | `channel.window_change` |
| Errors | regex-parse `ssh` stderr text | typed russh / io errors → reason strings |
| UID hack | `/etc/passwd` entry for 1001 | none — russh never calls `getpwuid` |

**Wire protocol is unchanged** (`connect`/`in`/`sz` in; `ready`/`err`/binary out),
so `web/src/app.svelte` needs no changes.

## Dependencies (Cargo.toml)

- `tokio` — `rt-multi-thread`, `macros`, `net`, `io-util`, `sync`
- `russh` — native SSH client (pin a recent version; `ring` crypto for easy musl static link)
- `axum` — with the `ws` feature (router + WebSocket)
- `rust-embed` — bake `web/dist` into the binary (debug mode reads from disk for dev)
- `mime_guess` — content-type for embedded assets
- `serde`, `serde_json` — parse client JSON frames
- `anyhow` / `thiserror` — error handling
- `tracing`, `tracing-subscriber` — logging

## Dockerfile (3 stages)

```dockerfile
# 1) frontend
FROM oven/bun:1 AS web
WORKDIR /web
COPY web/package.json web/bun.lock ./
RUN bun install --frozen-lockfile
COPY web/ ./
RUN bun run build                       # -> /web/dist

# 2) rust static binary (rust-embed bakes /web/dist at compile time)
FROM rust:1-alpine AS server
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY --from=web /web/dist ./web/dist
RUN cargo build --release --target x86_64-unknown-linux-musl

# 3) runtime — just the binary
FROM scratch
COPY --from=server /src/target/x86_64-unknown-linux-musl/release/termita /termita
ENV PORT=3000 HOST=0.0.0.0
EXPOSE 3000
ENTRYPOINT ["/termita"]
```

No node, no Bun, no `node_modules`, no `openssh-clients`, no `/etc/passwd` hack.
Runs as any UID → OpenShift random-UID safe. `scratch` needs no CA certs (SSH uses
host keys, not TLS PKI).

**Offline / in-cluster OCP builds:** if build pods have no internet, `cargo vendor`
the crates (+ `.cargo/config.toml`) and pre-build/commit `web/dist` or use a
bun/npm mirror. Simplest robust path: `docker build` locally and push the image to
the OpenShift registry so build-time internet isn't a constraint.

## Local development

- Backend: `cargo run` → serves `:3000` and `/ws`.
- Frontend: `cd web && bun run dev` → Vite dev server, proxies `/ws` → `:3000`
  (same as today). `rust-embed` debug mode serves `web/dist` from disk, so the
  backend doesn't need a recompile when the frontend rebuilds.

## Implementation steps (each with a verify)

1. **Restructure** — move `index.html`, `package.json`, `bun.lock`, `vite.config.js`,
   `svelte.config.js`, `src/{main.js,app.svelte}` into `web/`; set Vite `outDir: dist`.
   → verify: `cd web && bun run build` produces `web/dist/`.
2. **Scaffold Cargo** — `Cargo.toml` + `src/main.rs` (hello server).
   → verify: `cargo build` succeeds.
3. **serve.rs** — axum server, `rust-embed` static assets + SPA fallback, `/ws` route.
   → verify: `cargo run` serves `index.html` at `:3000`.
4. **ssh.rs** — russh connect + password (+ keyboard-interactive) auth + pty + shell;
   expose read/write/resize/close. → verify: connects to a known SSH host manually.
5. **bridge.rs** — ws loop: on `connect` validate + `ALLOWED_HOSTS`, open ssh, relay
   with `tokio::select!`, map errors to `{t:"err"}`, emit `{t:"ready"}`.
   → verify: full browser session end-to-end.
6. **main.rs** — read `PORT`/`HOST`/`ALLOWED_HOSTS`; start the server.
7. **Dockerfile + .dockerignore** → verify: `docker build`; `docker run -p 3000:3000`; connect.
8. **OpenShift UID check** → verify: `docker run --user 1000700000:0 …` still works
   (proves the `getpwuid` bug is gone).
9. **Cleanup & docs** — delete `server.js`, root `package.json`/`bun.lock`; update
   `README.md` and `doc/spec.md` to the Rust architecture.

## Acceptance criteria (ported from spec.md)

- Image builds; container serves `http://localhost:3000`.
- Correct creds → interactive remote shell; resize, Ctrl+C, arrows, tab, `vim`, `top` work.
- Wrong password → stays on the login form with "Authentication failed…", no terminal flash.
- Unreachable / unresolvable host → stays on the form with a clear reason.
- `csh`/`tcsh`/`zsh`/`fish` login shells connect normally (no bootstrap hack).
- Container runs under an arbitrary UID — no "No user exists for uid".
- Runtime image contains only the binary (no `ssh` client, no node).
- Frontend behavior unchanged (history pre-fill, Advanced port, password never stored).

## Risks / tradeoffs

- **russh API specifics** — pin an exact version; confirm `request_pty`/`request_shell`
  and the channel read/write API at implementation time (the crate moves fast).
- **crypto on musl/scratch** — prefer `ring`; if using `aws-lc` add build deps. Confirm
  fully static link. (No CA certs needed on `scratch` for SSH.)
- **keyboard-interactive-only servers** — include a KI fallback to match current parity.
- **MOTD now visible** (was hidden by the sentinel trick) — arguably more correct; can
  be suppressed later if undesired.
- **Multi-prompt / 2FA / key auth** — still unsupported (same as today).
- **Rewrite risk** in the subtle auth/error mapping — covered by the live-host
  acceptance checks above.

Estimated effort: ~1 day.
```
