<!-- src/app.svelte -->
<script>
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import { tick, untrack } from "svelte";
  import "@xterm/xterm/css/xterm.css";

  // Recent connections (host/user/port) are remembered in localStorage and
  // offered as a dropdown; the most recent pre-fills the form. The password is
  // never stored. (localStorage, not a cookie: these never need to reach the
  // server, so there's no reason to ship the SSH target on every request.)
  const HISTORY_KEY = "termita.history";
  const LAST_KEY = "termita.last"; // legacy single-entry key, used as a fallback
  function loadHistory() {
    try { return JSON.parse(localStorage.getItem(HISTORY_KEY)) || []; } catch { return []; }
  }
  function loadLegacy() {
    try { return JSON.parse(localStorage.getItem(LAST_KEY)) || {}; } catch { return {}; }
  }
  let history = $state(loadHistory());
  const seed = history[0] || loadLegacy();

  function saveHistory() {
    const rest = history.filter((h) => !(h.host === host && h.user === user && h.port === port));
    history = [{ host, user, port }, ...rest].slice(0, 6);
    try { localStorage.setItem(HISTORY_KEY, JSON.stringify(history)); } catch {}
  }

  let view = $state("form"); // "form" | "connecting" | "term"
  let error = $state("");
  let status = $state(""); // sub-status shown while connecting
  let live = $state(false);
  let host = $state(seed.host || "");
  let port = $state(seed.port || "22");
  let user = $state(seed.user || "");
  let password = $state("");
  let showPassword = $state(false);
  let capsOn = $state(false);
  let showAdvanced = $state(!!seed.port && seed.port !== "22");

  let termEl = $state(null);
  let hostEl = $state(null);
  let passwordEl = $state(null);

  let ws = null;
  let term = null;
  let fit = null;
  let pending = [];
  let onResize = null;

  // Focus the right field whenever the form is shown: the password if host/user
  // are already filled (return visit, or after a failed/canceled attempt), else
  // the host. Only re-runs on view change — host/user are read untracked so
  // typing in them doesn't steal focus back.
  $effect(() => {
    if (view !== "form") return;
    untrack(async () => {
      await tick();
      if (host && user && passwordEl) { passwordEl.focus(); passwordEl.select(); }
      else hostEl?.focus();
    });
  });

  function pickRecent(e) {
    const v = e.target.value;
    e.target.value = ""; // reset so re-picking the same entry fires again
    if (v === "") return;
    const h = history[Number(v)];
    if (!h) return;
    host = h.host;
    user = h.user;
    port = h.port || "22";
    showAdvanced = port !== "22";
    passwordEl?.focus();
  }

  function checkCaps(e) {
    if (e.getModifierState) capsOn = e.getModifierState("CapsLock");
  }

  function connect(e) {
    e.preventDefault();
    if (!host || !user) {
      error = "Host and username are required.";
      return;
    }
    error = "";
    status = "Contacting host…";
    saveHistory();
    view = "connecting";
    openWs();
  }

  function cancelConnect() {
    teardown();
    status = "";
    error = "Connection canceled.";
    view = "form";
  }

  // Soft reset back to the form — no full page reload, so the bundle isn't
  // re-fetched and the host/user/port stay pre-filled.
  function newConnection() {
    teardown();
    live = false;
    status = "";
    error = "";
    view = "form";
  }

  function openWs() {
    const proto = location.protocol === "https:" ? "wss" : "ws";
    ws = new WebSocket(`${proto}://${location.host}/ws`);
    ws.binaryType = "arraybuffer";

    ws.onopen = () => {
      ws.send(JSON.stringify({ t: "connect", host, port: Number(port) || 22, user, password, cols: 80, rows: 24 }));
    };
    ws.onmessage = (ev) => {
      if (typeof ev.data === "string") {
        let m;
        try { m = JSON.parse(ev.data); } catch { return; }
        if (m.t === "ready") {
          live = true;
          status = "";
          view = "term"; // terminal is created by the $effect below
        } else if (m.t === "status") {
          if (m.phase === "auth") status = "Authenticating…";
        } else if (m.t === "err") {
          error = m.reason || "Connection failed.";
          status = "";
          teardown();
          view = "form";
        }
        return;
      }
      const bytes = new Uint8Array(ev.data);
      if (term) term.write(bytes);
      else pending.push(bytes); // arrives just before the terminal mounts
    };
    ws.onclose = () => {
      if (view === "term") {
        live = false;
        term && term.write("\r\n\x1b[33m[disconnected]\x1b[0m\r\n");
      } else if (!error) {
        error = "Connection closed.";
        status = "";
        view = "form";
      }
    };
    ws.onerror = () => {
      if (view !== "term" && !error) {
        error = "Connection error.";
        status = "";
        view = "form";
      }
    };
  }

  function teardown() {
    if (ws) {
      ws.onopen = ws.onmessage = ws.onclose = ws.onerror = null;
      try { ws.close(); } catch {}
    }
    if (onResize) window.removeEventListener("resize", onResize);
    try { term && term.dispose(); } catch {}
    ws = null;
    term = null;
    fit = null;
    pending = [];
    onResize = null;
  }

  // Create the terminal once we're connected and the host div is mounted.
  $effect(() => {
    if (view === "term" && termEl && !term) startTerminal();
  });

  function startTerminal() {
    term = new Terminal({
      cursorBlink: true,
      fontFamily: "Consolas, 'Courier New', monospace",
      fontSize: 14,
      scrollback: 5000,
      theme: { background: "#1e1e1e", foreground: "#e0e0e0" },
    });
    fit = new FitAddon();
    term.loadAddon(fit);
    term.open(termEl);
    fit.fit();

    for (const b of pending) term.write(b);
    pending = [];

    const sendSize = () => {
      if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ t: "sz", cols: term.cols, rows: term.rows }));
      }
    };
    sendSize();
    term.onData((d) => {
      if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ t: "in", d }));
    });
    onResize = () => {
      fit.fit();
      sendSize();
    };
    window.addEventListener("resize", onResize);
    term.focus();
  }
</script>

{#if view !== "term"}
  <div class="login">
    <h1>termita</h1>
    <p class="sub">tiny browser terminal for hard-to-reach hosts</p>
    <form onsubmit={connect}>
      {#if history.length}
        <label class="sr-only" for="recent">Recent connections</label>
        <select id="recent" class="recent" onchange={pickRecent} disabled={view === "connecting"}>
          <option value="">Recent connections…</option>
          {#each history as h, i}
            <option value={i}>{h.user}@{h.host}{h.port && h.port !== "22" ? ":" + h.port : ""}</option>
          {/each}
        </select>
      {/if}

      <label class="sr-only" for="host">Host</label>
      <input id="host" bind:this={hostEl} placeholder="host  (e.g. 192.168.100.250)" bind:value={host} disabled={view === "connecting"} autocomplete="off" autocapitalize="off" spellcheck="false" />

      <label class="sr-only" for="user">Username</label>
      <input id="user" placeholder="username" bind:value={user} disabled={view === "connecting"} autocomplete="off" autocapitalize="off" spellcheck="false" />

      <label class="sr-only" for="password">Password</label>
      <div class="pw">
        <input id="password" bind:this={passwordEl} type={showPassword ? "text" : "password"} placeholder="password" bind:value={password} disabled={view === "connecting"} onkeydown={checkCaps} onkeyup={checkCaps} autocomplete="off" />
        <button type="button" class="reveal" onclick={() => (showPassword = !showPassword)} aria-label={showPassword ? "Hide password" : "Show password"} aria-pressed={showPassword} tabindex="-1" disabled={view === "connecting"}>
          {#if showPassword}
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20C5 20 1 12 1 12a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19"/><line x1="1" y1="1" x2="23" y2="23"/></svg>
          {:else}
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M1 12s4-7 11-7 11 7 11 7-4 7-11 7-11-7-11-7z"/><circle cx="12" cy="12" r="3"/></svg>
          {/if}
        </button>
      </div>
      {#if capsOn}<div class="caps" role="status">⇪ Caps Lock is on</div>{/if}

      <button type="button" class="adv-toggle" onclick={() => (showAdvanced = !showAdvanced)} aria-expanded={showAdvanced}>
        <span class="caret">{showAdvanced ? "▾" : "▸"}</span> Advanced
      </button>
      {#if showAdvanced}
        <label class="field">
          <span class="field-label">SSH port</span>
          <input placeholder="22" bind:value={port} disabled={view === "connecting"} inputmode="numeric" />
        </label>
      {/if}

      {#if view === "connecting"}
        <div class="connecting"><span class="spinner" aria-hidden="true"></span><span>{status || "Connecting…"}</span></div>
        <button type="button" class="btn ghost" onclick={cancelConnect}>Cancel</button>
      {:else}
        <button class="btn" type="submit">Connect</button>
      {/if}
      {#if error}<div class="err" role="alert">{error}</div>{/if}
    </form>
  </div>
{:else}
  <div class="app">
    <header>
      <span class="who"><span class="dot" class:on={live}></span>{user}@{host}</span>
      <button class="btn small" onclick={newConnection}>{live ? "Disconnect" : "New connection"}</button>
    </header>
    <div class="term" bind:this={termEl}></div>
  </div>
{/if}

<style>
  :global(html, body, #app) {
    margin: 0;
    height: 100%;
    background: #1e1e1e;
  }
  .login {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100vh;
    color: #ddd;
    font-family: system-ui, sans-serif;
  }
  .login h1 {
    margin: 0;
  }
  .sub {
    color: #888;
    margin: 4px 0 18px;
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 10px;
    width: 320px;
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }
  input {
    padding: 10px 12px;
    border: 1px solid #3a3a3a;
    border-radius: 6px;
    background: #262626;
    color: #eee;
    font-size: 14px;
  }
  input:disabled {
    opacity: 0.6;
  }
  .recent {
    padding: 9px 12px;
    border: 1px solid #3a3a3a;
    border-radius: 6px;
    background: #262626;
    color: #bbb;
    font-size: 13px;
  }
  .recent:disabled {
    opacity: 0.6;
  }
  .pw {
    position: relative;
    display: flex;
  }
  .pw input {
    flex: 1;
    padding-right: 40px;
  }
  .reveal {
    position: absolute;
    right: 6px;
    top: 50%;
    transform: translateY(-50%);
    display: flex;
    align-items: center;
    background: none;
    border: none;
    color: #9aa0a6;
    cursor: pointer;
    padding: 4px;
  }
  .reveal:hover {
    color: #ddd;
  }
  .reveal:disabled {
    color: #555;
    cursor: default;
  }
  .caps {
    color: #f59e0b;
    font-size: 12px;
    margin-top: -4px;
  }
  .adv-toggle {
    align-self: flex-start;
    background: none;
    border: none;
    padding: 2px 0;
    color: #9aa0a6;
    font: 13px system-ui, sans-serif;
    cursor: pointer;
  }
  .adv-toggle:hover {
    color: #ccc;
  }
  .caret {
    display: inline-block;
    width: 1em;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .field-label {
    font: 12px system-ui, sans-serif;
    color: #888;
  }
  .connecting {
    display: flex;
    align-items: center;
    gap: 10px;
    color: #ccc;
    font-size: 14px;
    padding: 2px 0;
  }
  .spinner {
    width: 16px;
    height: 16px;
    border: 2px solid #3a3a3a;
    border-top-color: #2563eb;
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .btn {
    background: #2563eb;
    color: #fff;
    padding: 10px 18px;
    border: none;
    border-radius: 6px;
    cursor: pointer;
    font-size: 15px;
  }
  .btn:disabled {
    opacity: 0.7;
    cursor: default;
  }
  .btn.ghost {
    background: none;
    border: 1px solid #3a3a3a;
    color: #ccc;
  }
  .btn.ghost:hover {
    border-color: #555;
  }
  .btn.small {
    padding: 4px 10px;
    font-size: 13px;
  }
  .err {
    color: #ef4444;
    font-size: 13px;
  }
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 4px 12px;
    background: #2d2d2d;
    color: #ccc;
    font: 13px system-ui, sans-serif;
  }
  .who {
    color: #8bc34a;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #777;
  }
  .dot.on {
    background: #22c55e;
  }
  .term {
    flex: 1;
    min-height: 0;
    padding: 6px;
  }
</style>
