# Termita — Specification

> tiny browser terminal for hard-to-reach hosts

## Goal

A browser-based terminal that connects to a **remote host over SSH**. The user
enters a host, username, and password in the browser; the server runs the OpenSSH
`ssh` client in a PTY and streams the **remote** shell back to the browser.

Termita is **only an SSH client** — it has no shell, no user accounts, and no
`sshd` of its own. Authentication is delegated entirely to the remote host's SSH
login. The container holds no user data; each connection is just an `ssh` process.

```text
Browser (Svelte 5 + xterm.js) ──WebSocket──▶ termita (Node + node-pty → ssh) ──▶ remote host shell
```

## Components

| Layer | Choice |
|---|---|
| Frontend | Svelte 5 + [xterm.js](https://xtermjs.org) (+ fit addon), bundled by **Vite via Bun** → `web/dist` |
| Tooling | **Bun** for dependency install / lockfile (`bun.lock`) and the frontend build |
| Backend | **Node** `http` + `ws` — serves the static bundle and a `/ws` WebSocket |
| Relay | `node-pty` spawns the OpenSSH `ssh` client; the browser sees the remote shell |
| Base image | build stage `ubi9/nodejs-20`; runtime stage `ubi9/nodejs-20-minimal` (~450 MB), runs as **UID 1001** |

### Why Node runs the server (not Bun)

`node-pty` drives the PTY master via Node's `net.Socket({fd})`, which Bun 1.3 does
not fully support — under Bun the spawned `ssh` receives `SIGHUP` and exits
immediately. Bun is still used for install + the Svelte build; **Node** runs the
small relay. Node is already present in the UBI9 base image, so nothing extra is added.

## Connection flow

1. The browser loads the connect form: **host / username / password**, with the
   SSH **port** tucked under an *Advanced* toggle (default `22`). The last
   host/username/port are persisted in `localStorage` and pre-filled on the next
   visit; the password is never stored.
2. On submit it opens `/ws` and (on open) sends a `connect` message with the
   credentials and the initial terminal size.
3. The server spawns:
   ```
   ssh -tt
       -o StrictHostKeyChecking=accept-new
       -o UserKnownHostsFile=/tmp/known_hosts
       -o ConnectTimeout=15
       -o NumberOfPasswordPrompts=1
       -o PreferredAuthentications=password,keyboard-interactive
       -p <port> <user>@<host>
       exec /bin/sh -c 'printf "\nTERMITA_READY_<nonce>\n"; exec "${SHELL:-/bin/bash}" -l'
   ```
   in a PTY (`cwd=/tmp`, `HOME=/tmp`, `TERM=xterm-256color`). The remote command
   is run by the user's **login shell**, so it is wrapped in `/bin/sh -c …`: the
   bootstrap uses POSIX syntax (`${SHELL:-…}`) that csh/tcsh cannot parse, so
   running it directly under a csh login shell would fail before the `exec`. The
   `/bin/sh` wrapper parses it, then `exec`s the user's real `$SHELL` as a login
   shell — bash/csh/tcsh/fish users all land on their normal shell.
4. When `ssh` prints its password prompt (`/[Pp]assword:\s*$/`), the server writes
   the password **once** to the PTY. The password is never echoed and never sent
   back to the browser; the server's copy is dropped immediately after.
5. **Establishment is deterministic:** the remote command prints a unique
   `TERMITA_READY_<nonce>` sentinel after a successful login, then `exec`s the
   user's login shell.
   - **Sentinel seen** → server sends `{t:"ready"}` and begins streaming output.
     All output up to and including the sentinel (SSH banner, password prompt,
     MOTD) is hidden so the user lands on a clean remote prompt.
   - **`ssh` exits before the sentinel** → server sends `{t:"err", reason}` and
     closes. The client stays on the login form and shows the reason
     (auth failure if a password was sent, otherwise the raw `ssh` connection error).
6. After `ready` it is a transparent relay: keystrokes → `ssh`, output → browser,
   plus terminal resize (forwarded to the remote PTY). When `ssh` exits (logout,
   dropped connection) the socket closes and the UI offers **New connection**.

**Nothing is stored**: the password is used once to authenticate, then discarded.

## Wire protocol

Client → server (JSON text frames):

- `{"t":"connect","host":…,"user":…,"port":22,"password":…,"cols":N,"rows":M}` — open the session
- `{"t":"in","d":"<keystrokes>"}` — input
- `{"t":"sz","cols":N,"rows":M}` — resize

Server → client:

- **Control** (JSON text frames): `{"t":"ready"}` | `{"t":"err","reason":"…"}`
- **Output** (binary frames): raw `ssh`/PTY bytes

The client distinguishes the two by frame type (text = control, binary = output)
and buffers any output that arrives before the terminal element mounts.

## Configuration (environment)

| Var | Default | Purpose |
|---|---|---|
| `PORT` | `3000` | listen port |
| `HOST` | `0.0.0.0` | bind address inside the container |
| `ALLOWED_HOSTS` | _(empty = any)_ | comma-separated allowlist of SSH targets |

## Security model

- **Client only** — the image installs `openssh-clients` (and `bash` for the shell
  it relies on); there is **no `sshd`**. You connect *out*, never *into* the container.
- **Password transport** — the password travels browser → server over the
  WebSocket. Run behind **TLS (https/wss)** in any real deployment.
- **Bind to localhost** (`-p 127.0.0.1:3000:3000`). The relay can reach any host the
  container can reach, so do not expose it unauthenticated on a network. Use
  `ALLOWED_HOSTS` to restrict targets.
- **No credential storage** — the password is fed to the `ssh` PTY once and then
  dropped; it is never written to disk, logs, or process arguments. The browser
  remembers the **host/username/port** in `localStorage` for convenience, but
  **never the password**.
- **Host keys** — `StrictHostKeyChecking=accept-new` (trust-on-first-use);
  `known_hosts` lives in `/tmp` and is ephemeral.
- The minimal base has no `/etc/passwd` entry for UID 1001; one is added
  (`termita:x:1001:0:termita:/tmp:/bin/bash`) so the `ssh` client's `getpwuid()` call
  succeeds (otherwise: *"No user exists for uid 1001"*).

## Acceptance criteria

The core relay items (✓) were verified against a live host. The shell-compat and
form-persistence items (☐) are by design from the latest changes and should be
re-checked against a live host before release.

- ✓ Image builds from UBI 9; container starts and serves `http://localhost:3000`.
- ✓ Correct credentials → interactive **remote** shell (resize, Ctrl+C, arrow keys,
  tab completion, `vim`, `top` all work — provided by the remote host's PTY).
- ✓ The `TERMITA_READY_` sentinel is hidden from the visible output.
- ✓ Wrong password → client **stays on the login form** with
  *"Authentication failed — check your username and password."* (no terminal flash).
- ✓ Unreachable / unresolvable host → stays on the form with the `ssh` error.
- ✓ Closing the connection ends the session; closing the container stops everything.
- ☐ A remote account whose **default login shell is `csh`/`tcsh`** (or fish/zsh/…)
  connects normally — the `/bin/sh -c` bootstrap wrapper avoids the old false
  *"Authentication failed"* that csh produced by choking on `${SHELL:-…}`.
- ☐ The connect form **pre-fills the last host / username / port** from
  `localStorage` and never stores the password; the SSH **port** is hidden under
  the *Advanced* toggle (default `22`), which auto-expands when the saved port ≠ 22.

## Build & run

```bash
docker build -t termita-ubi9 .
docker run --rm -it -p 127.0.0.1:3000:3000 termita-ubi9
# open http://localhost:3000 → enter host / username / password → Connect
```

Local development:

```bash
bun install
bun run start    # Node relay on :3000
bun run dev      # Vite dev server (open the URL it prints; proxies /ws → :3000)
```

## File layout

```text
termita/
├─ Dockerfile          multi-stage UBI9 build (Bun build → Node runtime)
├─ server.js           Node http + ws relay; spawns ssh in a PTY
├─ package.json        deps: ws, node-pty (runtime); svelte, vite (build)
├─ index.html          Vite entry
├─ src/
│  ├─ main.js          mounts the Svelte app
│  └─ app.svelte       connect form + xterm.js terminal
├─ vite.config.js / svelte.config.js
└─ doc/spec.md         this file
```

## Out of scope / superseded approaches

Earlier iterations were explored and **deliberately dropped** in favor of the
simple web-SSH relay above:

- **Shell *inside* the container** + `ssh` run manually by the user.
- **Keycloak (OIDC BFF) authentication**, server-side sessions, per-user home
  directories on a PVC.
- **Per-user isolation** (per-user pods or per-user OS uids).

These were removed because authenticating to the remote host *is* the
authentication, and the remote host's own OS enforces user identity and isolation —
so Termita needs none of that complexity. Re-introducing identity-aware access
(e.g., an allowlist tied to an IdP, or SSH-key/CA provisioning) would be a future,
separately-scoped effort.

## Known limitations

- The remote bootstrap is wrapped in `/bin/sh -c …` and then `exec`s the user's
  own `$SHELL`, so the default login shell can be anything that accepts `-l`
  (bash/csh/tcsh/zsh/ksh/fish). The only requirements are that `/bin/sh` exists on
  the remote host (universal on Unix) and the login prints no extra output between
  the sentinel and the prompt.
- Only single password / keyboard-interactive single-prompt auth is handled — no
  multi-step 2FA, and no SSH key / certificate auth yet.
