# Decision record — jump host (ProxyJump) support

> Why and how termita gained optional bastion routing. Past-tense context for
> future readers; the current behavior lives in `README.md` (How it works /
> Protocol) and the code in `src/ssh.rs` + `src/bridge.rs`.

## Context

termita could only reach hosts the relay container can dial **directly**. Many real
targets sit behind a **bastion / jump host** and are only reachable *through* it.
The sibling project *neos* already solved this with russh's
`channel_open_direct_tcpip` (`crates/neos-core/src/ssh/jump.rs`), so the mechanism
was known-good.

## Decision

Add **one optional field** under **Advanced**, entered as `user@jumper`
(`user@host[:port]`, user and port optional). When set, the session is tunneled
through that single bastion. **The same password authenticates both hops** — no
second credential, no key storage.

### Mechanism (cleaner than neos)

neos opens a `direct-tcpip` channel and then proxies it through a **local TCP
listener** because its backend connects by socket address. termita owns
`ssh::connect`, so it skips the listener entirely:

1. `client::connect` to the bastion; authenticate with the password.
2. `jump.channel_open_direct_tcpip(target, port, "127.0.0.1", 0)` → `.into_stream()`
   (a `ChannelStream`, which is `AsyncRead + AsyncWrite + Unpin + Send + 'static`).
3. `client::connect_stream(cfg, stream, Client)` runs the **target** session over the
   tunnel; authenticate the target with the same password; request PTY + shell.
4. The bastion `Handle` is retained in `Shell._jump` so the tunnel stays open for the
   session's lifetime.

Parsing lives in `bridge.rs::parse_jump`: user defaults to the target's username; a
missing/malformed port falls back to 22 so a typo never silently drops the jump.

## Security — why password-only, no browser key storage

We deliberately did **not** add SSH-key auth or store any key in the browser.
`localStorage` is plaintext and JS-readable, so an XSS / supply-chain compromise
would exfiltrate a long-lived, often-reused private key — strictly worse than
termita's existing "password used once, never persisted" posture. A stored password
is one host's credential (scoped, rotatable); a private key is high-value and
commonly reused. Reusing the existing single password for both hops keeps the jump
feature within the same threat model as before. If key auth is ever wanted, hold the
key **server-side** (configured path / agent, as neos does) or in memory for one
session — never in `localStorage`.

## Tradeoffs / scope

- **Single hop only** — matches the simple `user@jumper` UX; no chained jumps.
- **Same credentials for both hops** — by design; no per-hop username/password.
- The bastion is dialed directly by the relay, so it is subject to the same
  `ALLOWED_HOSTS` allowlist as the target.
- Host keys for the bastion are trust-on-first-use, like the target (termita has no
  persistent `known_hosts`).
