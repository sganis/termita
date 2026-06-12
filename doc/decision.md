# Decision record — Rust + russh rewrite

> Why termita's backend moved from Node (`node-pty` + spawned `ssh`) to a single
> Rust binary (`axum` + `russh`). Past-tense context for future readers; the
> current design lives in `spec.md` and `README.md`.

## Context

The backend was a Node `http` + `ws` relay that spawned the OpenSSH `ssh` client in
a PTY via `node-pty` and scraped its output. Deploying it to OpenShift was painful,
and the goal was to "build locally and deploy the built artifact" without an install
step in the image.

The three friction points, all traced to the deployment model rather than the code:

1. **`node-pty` is a native module with no Linux prebuilt** — it compiled from
   source via node-gyp, needing a C toolchain (and a Windows-built `node_modules`
   could not be copied into a Linux image).
2. **Build-time network** — the image downloaded Bun via `curl | bash` and ran
   `bun install`; OpenShift build pods often block outbound network.
3. **OpenShift random UID** — the `ssh` client called `getpwuid()` and failed under
   OpenShift's arbitrary UID with *"No user exists for uid …"*. A hardcoded
   `/etc/passwd` entry for UID 1001 did not cover the random UID.

## Decision

Rewrite the **backend** in Rust using `russh` (native, pure-Rust SSH client); keep
the Svelte + xterm.js **frontend** unchanged. Ship a static
`x86_64-unknown-linux-musl` binary on `FROM scratch`, with the frontend embedded via
`rust-embed`. Build in a three-stage Dockerfile (bun → cargo → scratch).

## Why this addressed the friction (and what it did *not* change)

- The win was **operational, not performance.** termita is an I/O-bound byte relay;
  under load the per-session `ssh`/SSH work dominates, so Rust is not meaningfully
  faster or more scalable. It was chosen for deployment simplicity.
- **`node-pty` → russh PTY:** the PTY is part of the SSH protocol, so there is no
  native module and no node-gyp. The Windows-vs-Linux `node_modules` problem is gone
  because there is no `node_modules` — just one static binary.
- **Native SSH deleted the hacks:** real auth pass/fail and a real PTY channel
  removed the sentinel-scrape, the one-shot password injection, and the
  `/bin/sh -c` csh/tcsh bootstrap.
- **Random-UID bug fixed for free:** russh never calls `getpwuid`, so any UID works
  and the `/etc/passwd` workaround was removed.
- **Crypto backend:** russh's default `aws-lc-rs` needs NASM/cmake (hostile to both
  Windows and musl), so the build uses the `ring` backend instead.

## Known tradeoffs / follow-ups

- A Linux build step is still required (done in the cargo Docker stage); only the
  *artifact* is install-free.
- Banner/MOTD now shows on connect (the old sentinel trick hid it) — normal terminal
  behavior, kept intentionally.
- Auth is still single password / keyboard-interactive only; no 2FA or key auth.
- Host keys are trust-on-first-use, not pinned.
- For an offline in-cluster OpenShift build, vendor the cargo crates and pre-build
  `web/dist` (or build the image where network is available and push it).
