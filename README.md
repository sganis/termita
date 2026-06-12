# Termita

> tiny browser terminal for hard-to-reach hosts

A browser terminal that SSHes into a remote host. You type a host, username, and
password in the browser; the server runs the `ssh` client in a PTY and streams the
**remote** shell back to [xterm.js](https://xtermjs.org).

```text
Browser (Svelte 5 + xterm.js) ──WebSocket──▶ termita (Node + node-pty → ssh) ──▶ remote host shell
```

Termita is **only an SSH client** — it has no shell, accounts, or `sshd` of its own.
Authentication is the remote host's own SSH login. Base image: Red Hat UBI 9.

## Stack

- **Frontend:** Svelte 5 + xterm.js (fit addon), bundled by **Vite via Bun** → `web/dist`.
- **Tooling:** **Bun** for install/lockfile (`bun.lock`) and the build.
- **Backend:** **Node** (`http` + `ws`) serves the bundle and a `/ws` WebSocket.
- **Relay:** `node-pty` spawns the OpenSSH `ssh` client; the browser sees the remote shell.
- **Image:** built on `ubi9/nodejs-20`, runs on `ubi9/nodejs-20-minimal` (~450 MB), as UID 1001.

## How it works

1. The browser loads the connect form (host / username / password; the SSH port
   lives under **Advanced**). The last host/username/port are remembered in the
   browser's `localStorage` and pre-filled on the next visit — the password is not.
2. On submit it opens `/ws` and sends `{t:"connect", host, user, port, password, cols, rows}`.
3. The server spawns `ssh -tt … user@host` in a PTY. When `ssh` prints its password
   prompt, the server types the password **once** (never echoed, never sent back to
   the browser), then drops it. The remote bootstrap runs via `/bin/sh -c` and then
   `exec`s the user's own login shell, so it works whatever that shell is
   (bash/csh/tcsh/fish/…) — see the note in `doc/spec.md`.
4. After that it's a transparent relay: keystrokes → `ssh`, output → browser, plus
   terminal resize. When `ssh` exits (logout, wrong password, unreachable host) the
   socket closes and the UI shows a "New connection" button.

Nothing is stored: the password is used once to authenticate and discarded.

## Build & run

```bash
docker build -t termita-ubi9 .
docker run --rm -it -p 127.0.0.1:3000:3000 termita-ubi9
# open http://localhost:3000 → enter host / username / password → Connect
```

## Local development

```bash
bun install
bun run start    # terminal 1: Node relay on :3000
bun run dev      # terminal 2: Vite dev server (open the URL it prints; proxies /ws → :3000)
```

> The relay runs on **Node**, not Bun: `node-pty` drives the PTY master via Node's
> `net.Socket({fd})`, which Bun 1.3 doesn't fully support (the spawned `ssh` gets
> `SIGHUP`). Bun still does install + the Svelte build; Node is already in the UBI9 base.

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
- Host keys: `StrictHostKeyChecking=accept-new` (trust-on-first-use); `known_hosts`
  lives in `/tmp` and is ephemeral.

## Protocol

Client → server (JSON text frames):

- `{"t":"connect","host":…,"user":…,"port":22,"password":…,"cols":N,"rows":M}` — open the SSH session
- `{"t":"in","d":"<keystrokes>"}` — input
- `{"t":"sz","cols":N,"rows":M}` — resize

Server → client: raw `ssh`/PTY output (text frames).
