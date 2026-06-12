# termita — improvements plan

Status snapshot and a prioritized plan for what to improve next. Grounded in the
current code (commit `5c4eb3a`); file:line references point at the evidence.

## Status snapshot

termita does its core job well and is small and clean:

- **Backend** — 4 focused Rust files (`main.rs`, `serve.rs`, `bridge.rs`, `ssh.rs`,
  all well under the 500-line budget), native `russh` SSH, optional jump host,
  `ALLOWED_HOSTS` allowlist, deterministic auth/error mapping. `cargo test` is green
  (13 unit tests).
- **Frontend** — Svelte 5 + xterm.js: connect form, recent-connection history,
  password reveal, caps-lock hint, Advanced (jump + port), resize relay.
- **Docs/deploy** — README, spec, decision record, deploy guide; a static-musl scratch
  image plus a scripted OpenShift rollout (`deploy/deploy.sh`).

The gaps are not in the happy path — they are in **operability, hardening, auth
breadth, and live-host test coverage**. The project is feature-complete for "type a
password, get a remote shell," but thin everywhere a real multi-user deployment cares.

### Biggest risks today

1. **The relay is an unauthenticated open SSH proxy.** Anyone who can load the page can
   attempt SSH to any allowed host. `/ws` has no gate (`serve.rs:35`); the only control
   is `ALLOWED_HOSTS`. The README says "don't expose it unauthenticated" — that should
   be enforceable, not just advised.
2. **Host keys are accepted blindly** — `check_server_key` always returns `Ok(true)`
   (`ssh.rs:55`). No pinning, no fingerprint shown → MITM-able.
3. **No observability** — `println!` only (`main.rs:28`); no per-connection logging,
   no metrics. A deployed instance is a black box.
4. **The spec's live-host acceptance items are still unchecked** (`doc/spec.md:168-178`)
   — auth success/failure, real shell, csh, arbitrary UID — none are tested in CI.

## Improvement catalog

Effort: S (hours) · M (1–2 days) · L (multi-day). Impact: ★–★★★.

### Security / hardening

| ID | Improvement | Why / evidence | Effort | Impact |
|----|-------------|----------------|--------|--------|
| S1 | **Optional access gate on `/ws`** — shared token or basic auth via env (e.g. `ACCESS_TOKEN`). | No auth in front of the relay (`serve.rs:35`); it's an open proxy to `ALLOWED_HOSTS`. A simple gate ≠ the rejected per-user IdP. | M | ★★★ |
| S2 | **Host-key pinning / fingerprint** — verify against an optional `KNOWN_HOSTS`, and surface the fingerprint to the UI on first use (accept-on-confirm). | `check_server_key` blindly trusts any key (`ssh.rs:55`). | M | ★★★ |
| S3 | **Rate limiting + concurrent-session cap** (per client IP and global). | Nothing bounds connection/auth attempts or live sessions → password brute-force and DoS vectors. Unbounded SSH per `/ws`. | M | ★★ |
| S4 | **Smarter `ALLOWED_HOSTS`** — CIDR / glob, and match on the resolved address, not just the client-supplied string. | Exact string compare only (`bridge.rs:41-43`); a hostname alias can sidestep an IP allowlist. | M | ★★ |

### Robustness / observability

| ID | Improvement | Why / evidence | Effort | Impact |
|----|-------------|----------------|--------|--------|
| O1 | **Structured logging with `tracing`** — per-connection: client IP, target host/user, outcome, duration. Never log the password. | Only `println!` today (`main.rs:28`); zero connection visibility. | S–M | ★★★ |
| O2 | **Dedicated `/healthz`** instead of probing `/`. | Probes hit `/` and ship index.html (`deploy/openshift.yaml:80-88`); a tiny 200 is cheaper and clearer. | S | ★ |
| O3 | **Graceful shutdown** (SIGTERM → drain) so OpenShift rollouts don't cut live shells abruptly. | `axum::serve` has no shutdown signal (`main.rs:29`). | S | ★★ |
| O4 | **WebSocket frame-size / input bounds.** | No explicit limits on inbound frames. | S | ★ |

### Auth features

| ID | Improvement | Why / evidence | Effort | Impact |
|----|-------------|----------------|--------|--------|
| F1 | **True keyboard-interactive (2FA / OTP)** — relay each prompt to the browser instead of answering every prompt with the password. | Today every prompt is answered with the password (`ssh.rs:164-166`), so an OTP challenge fails. | L | ★★★ |
| F2 | **SSH key / certificate auth** (uploaded key or agent). | Password / keyboard-interactive only (`doc/decision.md:54`). | L | ★★ |
| F3 | **Emit an `auth` status before authenticating.** | The frontend already renders `phase:"auth"` ("Authenticating…", `app.svelte:131`) but the backend only ever sends `"jump"` (`bridge.rs:93`) — a dead branch and a free UX win. | S | ★ |
| F4 | **Multi-hop ProxyJump** (chain >1 bastion). | Only a single jump is supported (`ssh.rs:76-96`). | M | ★ |

### Quality / testing

| ID | Improvement | Why / evidence | Effort | Impact |
|----|-------------|----------------|--------|--------|
| Q1 | **Live-sshd integration test in CI** (containerized openssh-server): auth success/failure, login shell, resize, csh shell, arbitrary UID. | Closes the unchecked `☐` acceptance items (`doc/spec.md:168-178`); highest confidence gain. | M–L | ★★★ |
| Q2 | **Type the relay control frames** into an enum, like `Connect`. | `on_text` re-parses a `serde_json::Value` by hand (`bridge.rs:146-164`); an enum is safer and matches the existing pattern. | S | ★ |
| Q3 | **Frontend component tests** (vitest is already available) for history/focus/jump-parse logic. | No frontend tests exist. | M | ★ |

### Ops / release

| ID | Improvement | Why / evidence | Effort | Impact |
|----|-------------|----------------|--------|--------|
| R1 | **Keep the committed binary honest** — a script/`make` target to rebuild `deploy/termita`, or a CI check that it matches `HEAD` (or stop committing it and always pull from CI). | A committed binary silently drifts from source. | S | ★★ |
| R2 | **Release hygiene** — tag `v0.1.0`, keep CHANGELOG current. | Version is `0.1.0` (`Cargo.toml:4`) with no tags. | S | ★ |

## Recommended sequence

**Phase 0 — quick wins (a few hours):** F3, O2, Q2, R2.
Each is small, removes a wart, and needs no new dependency.

**Phase 1 — make it safe to actually deploy (a few days):** O1 (tracing), S1 (access
gate), S3 (rate limit + session cap), O3 (graceful shutdown).
This is the highest-leverage block — it turns "demo" into "deployable behind TLS."

**Phase 2 — security depth + confidence (a few days):** S2 (host-key pinning), S4
(`ALLOWED_HOSTS` matching), Q1 (live-sshd CI → finally check off the spec's `☐` items).

**Phase 3 — auth breadth (larger):** F1 (interactive 2FA), F2 (key/cert auth), then
F4 and Q3 as appetite allows.

## Architectural note

Per the project's file-budget rule (300–500 lines, hard 600), several of these warrant
their own small modules rather than swelling existing files: `src/log.rs` (O1),
`src/gate.rs` (S1), `src/hostkey.rs` (S2), `src/limit.rs` (S3). `bridge.rs` (263 lines)
and `ssh.rs` (233) have little headroom for F1/F2 inline.

## Explicitly out of scope

Per `doc/spec.md` and `doc/decision.md`, these were deliberately rejected and should not
be reintroduced under "improvements":

- A shell *inside* the container (termita is only an SSH client).
- OIDC/Keycloak with server-side sessions and per-user PVCs; per-user isolation. The
  remote host's own OS is the identity/isolation boundary. (S1's token gate is a
  lightweight access control, not identity-aware multi-tenancy.)
