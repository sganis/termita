// server.js
// Web SSH relay. The browser collects host/user/password; this server spawns
// `ssh` in a PTY and streams the REMOTE shell to xterm.js. There is no shell or
// account in the container — authentication is the remote host's own SSH login.
//
// Server → client framing:
//   - control: JSON text frames  {t:"ready"} | {t:"err", reason}
//   - output:  raw PTY bytes as BINARY frames
//
// To know auth succeeded deterministically (rather than guessing from timing),
// ssh runs a remote command that prints a unique sentinel right before exec'ing
// the login shell. Seeing the sentinel = established; ssh exiting before it =
// failure (wrong password / unreachable). Output before the sentinel (banner,
// password prompt, MOTD) is hidden so the user gets a clean shell.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { join, normalize, extname } from "node:path";
import { fileURLToPath } from "node:url";
import { randomBytes } from "node:crypto";
import { WebSocketServer } from "ws";
import pty from "node-pty";

const ROOT = fileURLToPath(new URL(".", import.meta.url));
const DIST = join(ROOT, "web", "dist");
const PORT = Number(process.env.PORT ?? 3000);
const HOST = process.env.HOST ?? "0.0.0.0";
const ALLOWED = (process.env.ALLOWED_HOSTS ?? "").split(",").map((s) => s.trim()).filter(Boolean);

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".css": "text/css",
  ".map": "application/json",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
};

const http = createServer(async (req, res) => {
  const url = new URL(req.url, `http://${req.headers.host}`);
  const rel = url.pathname === "/" ? "index.html" : url.pathname.replace(/^\/+/, "");
  let full = normalize(join(DIST, rel));
  if (!full.startsWith(DIST)) {
    res.writeHead(403).end("forbidden");
    return;
  }
  let body;
  try {
    body = await readFile(full);
  } catch {
    full = join(DIST, "index.html");
    body = await readFile(full);
  }
  res.writeHead(200, { "content-type": MIME[extname(full)] ?? "application/octet-stream" });
  res.end(body);
});

function reasonFromOutput(s) {
  const m =
    s.match(/ssh:[^\r\n]+/i) ||
    s.match(/(Could not resolve hostname|Connection refused|Connection timed out|No route to host|Operation timed out|Permission denied)[^\r\n]*/i);
  return m ? m[0].replace(/\s+$/, "") : "";
}

const wss = new WebSocketServer({ server: http, path: "/ws" });

wss.on("connection", (ws) => {
  let term = null;
  const sendErr = (reason) => {
    try {
      ws.send(JSON.stringify({ t: "err", reason }));
      ws.close();
    } catch {}
  };

  ws.on("message", (raw) => {
    let msg;
    try {
      msg = JSON.parse(raw.toString());
    } catch {
      return;
    }

    if (msg.t === "connect" && !term) {
      const host = String(msg.host || "").trim();
      const user = String(msg.user || "").trim();
      const port = Number(msg.port) || 22;
      let password = typeof msg.password === "string" ? msg.password : "";
      if (!host || !user) return sendErr("Host and username are required.");
      if (ALLOWED.length && !ALLOWED.includes(host)) return sendErr(`Host not allowed: ${host}`);

      const nonce = randomBytes(8).toString("hex");
      const sentinel = "TERMITA_READY_" + nonce;
      // Print the sentinel after login, then become the user's login shell.
      // The bootstrap runs via `/bin/sh -c` so it parses identically regardless
      // of the user's login shell. csh/tcsh choke on POSIX `${VAR:-default}`
      // ("Bad : modifier in $ (-).") — run directly under a csh login shell the
      // bootstrap would error out before the `exec`, and ssh would exit right
      // after auth, which we'd misreport as an auth failure. `/bin/sh` runs the
      // bootstrap, then exec's the user's real `$SHELL` (from /etc/passwd) as a
      // login shell — so csh/tcsh/fish users all land on their normal shell.
      const remoteCmd =
        "exec /bin/sh -c 'printf \"\\n" + sentinel + "\\n\"; exec \"${SHELL:-/bin/bash}\" -l'";

      term = pty.spawn(
        "ssh",
        [
          "-tt",
          "-o", "StrictHostKeyChecking=accept-new",
          "-o", "UserKnownHostsFile=/tmp/known_hosts",
          "-o", "ConnectTimeout=15",
          "-o", "NumberOfPasswordPrompts=1",
          "-o", "PreferredAuthentications=password,keyboard-interactive",
          "-p", String(port),
          `${user}@${host}`,
          remoteCmd,
        ],
        { name: "xterm-256color", cols: Number(msg.cols) || 80, rows: Number(msg.rows) || 24, cwd: "/tmp", env: { ...process.env, TERM: "xterm-256color", HOME: "/tmp" } },
      );

      let phase = "connecting"; // connecting | ready | failed
      let pre = "";
      let scan = "";
      let pwSent = false;

      const fail = (reason) => {
        if (phase !== "connecting") return;
        phase = "failed";
        sendErr(reason);
        try { term.kill(); } catch {}
      };

      term.onData((d) => {
        if (phase === "ready") {
          if (ws.readyState === ws.OPEN) ws.send(Buffer.from(d));
          return;
        }
        pre += d;
        scan = (scan + d).slice(-2048);

        if (!pwSent && /[Pp]assword:\s*$/.test(scan)) {
          pwSent = true;
          term.write(password + "\n");
          password = null;
          // Tell the client we've moved from "contacting" to "authenticating".
          try { if (ws.readyState === ws.OPEN) ws.send(JSON.stringify({ t: "status", phase: "auth" })); } catch {}
          return;
        }
        const idx = pre.indexOf(sentinel);
        if (idx !== -1) {
          phase = "ready";
          try {
            ws.send(JSON.stringify({ t: "ready" }));
            const rest = pre.slice(idx + sentinel.length).replace(/^\r?\n/, "");
            if (rest && ws.readyState === ws.OPEN) ws.send(Buffer.from(rest));
          } catch {}
          pre = "";
        }
      });

      term.onExit(() => {
        if (phase === "connecting") {
          const sshErr = reasonFromOutput(pre);
          if (/Permission denied/i.test(sshErr)) {
            // ssh rejected the password.
            fail("Authentication failed — check your username and password.");
          } else if (sshErr) {
            // A concrete ssh error: unresolvable host, refused, timed out, …
            fail(sshErr);
          } else if (pwSent) {
            // The password was accepted but the readiness sentinel never arrived,
            // and ssh reported no error — so login succeeded but the remote shell
            // didn't run our startup command. Happens with restricted / non-POSIX
            // shells that reject `exec` (e.g. Rebex's virtual shell, appliances).
            // This is NOT an auth failure, so don't claim it is.
            fail("Logged in, but the remote shell didn't start a session (it rejected the startup command).");
          } else {
            fail("Could not connect to the host.");
          }
        }
        try { ws.close(); } catch {}
      });
    } else if (msg.t === "in" && term) {
      term.write(msg.d);
    } else if (msg.t === "sz" && term && msg.cols > 0 && msg.rows > 0) {
      term.resize(msg.cols, msg.rows);
    }
  });

  ws.on("close", () => {
    try {
      term && term.kill();
    } catch {}
  });
});

http.listen(PORT, HOST, () => console.log(`termita web-ssh on http://${HOST}:${PORT}`));
