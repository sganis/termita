# Changelog

All notable changes to this project are documented here. Format based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- Optional **jump host (ProxyJump)**: enter `user@jumper` under **Advanced** to reach
  a target that's only accessible through a bastion. The relay connects and
  authenticates to the bastion, opens a direct-tcpip tunnel, and runs the target SSH
  session over it. The same password authenticates both hops; the jump field is
  remembered in `localStorage` (never the password) and is subject to `ALLOWED_HOSTS`.

### Changed
- Rewrote the backend from Node (`node-pty` + a spawned OpenSSH client) to a single
  Rust binary (`axum` + native pure-Rust `russh`). The Svelte + xterm.js frontend is
  unchanged and the WebSocket wire protocol is identical.
- The runtime image is now a static `x86_64-unknown-linux-musl` binary on
  `FROM scratch` (~3.4 MB), with the frontend embedded via `rust-embed`. No Node,
  Bun, `node_modules`, or `ssh` client in the image.
- Moved the frontend into `web/`.

### Fixed
- OpenShift random-UID startup failure (*"No user exists for uid …"*): native SSH
  never calls `getpwuid`, so the container runs under any UID without an
  `/etc/passwd` workaround.

### Removed
- The Node relay (`server.js`), `node-pty`, and the spawned OpenSSH client, along
  with their sentinel-scrape / password-injection / `/bin/sh -c` shell bootstrap.
