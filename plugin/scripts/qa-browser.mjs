#!/usr/bin/env node
// qa-browser.mjs: drives a headless system Chrome-family browser over the DevTools protocol (CDP) for the QA step
// (docs/run-folder.md, "QA"). Node 22+, no packages: it uses the global WebSocket.
//
//   node qa-browser.mjs --out <folder> [--steps <file>] [--timeout <ms>]   (steps: JSON lines on stdin, or a file)
//
// Steps, one JSON object per line, one JSON result line per step on stdout:
//   {"op":"open","url":"http://127.0.0.1:1530/"}
//   {"op":"click","selector":"button.start"}  or  {"op":"click","text":"Start"}
//   {"op":"type","selector":"input","text":"hello"}   (selector optional: types into the focused element)
//   {"op":"press","key":"Enter"}                       (real key events: Enter, Escape, Tab, Space, arrows, PageUp,
//                                                       PageDown, Home, End, Backspace, Delete, letters, digits)
//   {"op":"press","key":"Meta+K"}  or  {"op":"press","key":"K","modifiers":["meta"]}   (alt, ctrl, meta, shift)
//   {"op":"scroll","selector":".log"}  or  {"op":"scroll","text":"Receipt"}  or  {"op":"scroll","by":600}
//                                                       (an element into view, or pixels down: "by" is a number or {"x":0,"y":600})
//   {"op":"select","selector":"select#theme","label":"Light"}  or  ...,"value":"light"   (sets it, fires input and change;
//                                                       a missing option fails the step naming it)
//   {"op":"wait-text","text":"Planning","timeoutMs":5000}
//   {"op":"text","selector":"main"}                    (selector optional: the whole page)
//   {"op":"screenshot","name":"S1.1"}                  (writes <out>/S1.1.png)
//   {"op":"screenshot","name":"S1.2","fullPage":true}  (the whole page, beyond the viewport; same 5 MB cap, no file when over)
//
// Page errors print as they are seen, between the step lines:
//   {"event":"page-error","kind":"exception"|"console","message":"...","step":3}
// (an uncaught exception, a console.error call, or an error entry of the browser's log; step 0 is before the first step)
// A failed load or an error response prints {"event":"page-error","kind":"network","message":"404 Script","url":"...","step":3}; an error
// entry of the browser's log carries "url" when the browser names one. In web mode only a failed or error response of the page's own
// document (same origin as the last opened URL) prints a network line: 4xx fetch or image responses on real sites are normal.
//
// Web mode (web QA runs, docs/run-folder.md "Web QA mode"): on only when $DCV2_RUN_DIR holds a meta.json with mode "web-qa" and a
// webqa-target.json. Then the allowlist of that file (and nothing else: no step, argument or environment variable can add to it)
// decides which URLs open, the browser resolves only the allowlist hosts, every navigation is checked again, screenshots go to
// <run>/webqa/shots, and a type step may carry {"secret":"username"|"password"} (the driver reads the test account itself, never
// prints it, and types the password only into an input[type=password]).
//
// Exit: 0 all steps ok, 1 a step failed, 2 refused or no step read (non-loopback or non-allowlisted URL, bad out folder, bad step, an
// empty steps list), 3 no Chrome-family browser found. The browser runs in its own process group; it is killed on exit, on signals, and by a detached
// watchdog when this process dies by SIGKILL.
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const MAX_PNG_BYTES = 5 * 1024 * 1024;
const MAX_TEXT = 20000;
const BROWSERS = [
  ["Google Chrome", "Google Chrome"],
  ["Chromium", "Chromium"],
  ["Microsoft Edge", "Microsoft Edge"],
  ["Brave Browser", "Brave Browser"],
];

// Values the driver must never print (the web QA test account): replaced in every string it outputs.
const SECRETS = new Set();
function redact(value) {
  if (typeof value === "string") {
    let text = value;
    for (const secret of SECRETS) text = text.split(secret).join("[redacted]");
    return text;
  }
  if (Array.isArray(value)) return value.map(redact);
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, redact(v)]));
  return value;
}
function out(obj) {
  process.stdout.write(JSON.stringify(redact(obj)) + "\n");
}
function refuse(message, code = 2) {
  process.stderr.write(`qa-browser: ${redact(message)}\n`);
  process.exit(code);
}

/** Only http on 127.0.0.1, localhost or [::1] (a URL parse, so localhost.evil.com is refused). */
export function isLoopbackUrl(value) {
  let url;
  try {
    url = new URL(String(value));
  } catch {
    return false;
  }
  if (url.protocol !== "http:" || url.username || url.password) return false;
  return ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname.toLowerCase());
}

/** One allowlist entry parsed: an http(s) origin plus an optional path prefix. Null for anything else. */
export function parseAllowEntry(entry) {
  if (typeof entry !== "string" || /[\\\s\u0000-\u001f?#]/.test(entry)) return null;
  let url;
  try {
    url = new URL(entry);
  } catch {
    return null;
  }
  if (!["http:", "https:"].includes(url.protocol) || url.username || url.password || url.search || url.hash || !url.hostname) return null;
  return { scheme: url.protocol, host: url.hostname.toLowerCase(), port: url.port || (url.protocol === "https:" ? "443" : "80"), prefix: url.pathname.replace(/\/+$/, "") };
}

/**
 * Whether `value` may be opened for an allowlist of entries (an http or https origin, optionally with a path prefix). The URL is
 * parsed: scheme, host and effective port must be equal, and the path must be the prefix or below it at a `/` boundary. Userinfo,
 * any other scheme, backslashes and white space are refused, so a lookalike host (app.example.com.evil.com) never matches.
 * The same cases (fixtures/webqa/urls.json) are run against src-tauri's url_allowed.
 */
export function isAllowedUrl(value, allowlist) {
  const text = String(value);
  if (/[\\\s\u0000-\u001f]/.test(text)) return false;
  let url;
  try {
    url = new URL(text);
  } catch {
    return false;
  }
  if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) return false;
  const host = url.hostname.toLowerCase();
  const port = url.port || (url.protocol === "https:" ? "443" : "80");
  return (allowlist || []).some((entry) => {
    const e = typeof entry === "string" ? parseAllowEntry(entry) : entry;
    if (!e || e.scheme !== url.protocol || e.host !== host || e.port !== port) return false;
    return e.prefix === "" || url.pathname === e.prefix || url.pathname.startsWith(e.prefix + "/");
  });
}

/** Web mode: the target of the app's web QA run folder, or null. Only the environment of the process (set by the app) decides. */
export function webTarget(env = process.env) {
  const runDir = env.DCV2_RUN_DIR;
  if (!runDir) return null;
  try {
    const meta = JSON.parse(fs.readFileSync(path.join(runDir, "meta.json"), "utf8"));
    if (!meta || meta.mode !== "web-qa") return null;
    const target = JSON.parse(fs.readFileSync(path.join(runDir, "webqa-target.json"), "utf8"));
    const entries = Array.isArray(target.allowlist) ? target.allowlist : [];
    const parsed = entries.map(parseAllowEntry);
    if (!parsed.length || parsed.some((e) => !e)) refuse("webqa-target.json: the allowlist needs at least one valid http(s) entry and no invalid one");
    return { runDir, allowlist: parsed, hosts: [...new Set(parsed.map((e) => e.host))] };
  } catch (e) {
    if (e && e.code === "ENOENT") return null;
    if (e instanceof SyntaxError) refuse("web QA: meta.json or webqa-target.json is not valid JSON");
    throw e;
  }
}

/** The folder screenshots may go to: <DCV2_RUN_DIR>/reviews/qa, or a standalone results folder in the temp dir. */
export function allowedOut(outDir, env = process.env) {
  const resolved = path.resolve(outDir);
  const real = (p) => {
    try {
      return fs.realpathSync(p);
    } catch {
      return path.resolve(p);
    }
  };
  const runDir = env.DCV2_RUN_DIR;
  if (runDir && webTarget(env)) return real(resolved) === real(path.join(runDir, "webqa", "shots"));
  if (runDir) {
    return real(path.dirname(resolved)) === real(path.join(runDir, "reviews")) && path.basename(resolved) === "qa";
  }
  const tmp = real(os.tmpdir());
  const parent = real(path.dirname(resolved));
  return (parent === tmp || parent.startsWith(tmp + path.sep)) && path.basename(resolved).startsWith("dcv2-qa-");
}

function findBrowser(env = process.env) {
  if (env.DCV2_QA_BROWSER) {
    // For tests only, like state.sh's test mode.
    if (!/(^|[\s,])auto-approve([\s,]|$)/.test(env.DCV2_TEST_MODE || "")) {
      refuse("DCV2_QA_BROWSER is for tests only (set DCV2_TEST_MODE=auto-approve)");
    }
    return fs.existsSync(env.DCV2_QA_BROWSER) ? env.DCV2_QA_BROWSER : null;
  }
  for (const [app, bin] of BROWSERS) {
    const file = `/Applications/${app}.app/Contents/MacOS/${bin}`;
    if (fs.existsSync(file)) return file;
  }
  return null;
}

function parseArgs(argv) {
  const args = { out: null, steps: null, timeout: 30000 };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--out") args.out = argv[++i];
    else if (a === "--steps") args.steps = argv[++i];
    else if (a === "--timeout") args.timeout = Number(argv[++i]) || 30000;
    else refuse(`unknown argument ${a}`);
  }
  if (!args.out) refuse("--out <folder> is required");
  return args;
}

async function readSteps(file) {
  const text = file ? fs.readFileSync(file, "utf8") : await new Promise((resolve) => {
    let data = "";
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", (c) => (data += c));
    process.stdin.on("end", () => resolve(data));
  });
  const steps = [];
  for (const line of text.split("\n")) {
    if (!line.trim()) continue;
    try {
      steps.push(JSON.parse(line));
    } catch {
      refuse(`not a JSON step: ${line.slice(0, 60)}`);
    }
  }
  return steps;
}

const OPS = new Set(["open", "click", "type", "press", "scroll", "select", "wait-text", "text", "screenshot"]);

// CDP modifier bits.
const MODIFIER_BITS = { alt: 1, ctrl: 2, meta: 4, shift: 8 };
const NAMED_KEYS = {
  Enter: { code: "Enter", vk: 13, text: "\r" },
  Escape: { code: "Escape", vk: 27 },
  Tab: { code: "Tab", vk: 9, text: "\t" },
  " ": { code: "Space", vk: 32, text: " " },
  Space: { key: " ", code: "Space", vk: 32, text: " " },
  ArrowLeft: { code: "ArrowLeft", vk: 37 },
  ArrowUp: { code: "ArrowUp", vk: 38 },
  ArrowRight: { code: "ArrowRight", vk: 39 },
  ArrowDown: { code: "ArrowDown", vk: 40 },
  PageUp: { code: "PageUp", vk: 33 },
  PageDown: { code: "PageDown", vk: 34 },
  End: { code: "End", vk: 35 },
  Home: { code: "Home", vk: 36 },
  Backspace: { code: "Backspace", vk: 8 },
  Delete: { code: "Delete", vk: 46 },
};

/**
 * The CDP key events of a press step, or null when the key is not one the table covers. `key` may carry modifiers
 * ("Meta+K"); `modifiers` adds more. Each event has `key`, `code`, `windowsVirtualKeyCode`, `nativeVirtualKeyCode`
 * and the modifier bitmask; a printable key without Ctrl/Meta/Alt also carries its `text`.
 */
export function keyEvents(step) {
  const parts = String(step.key ?? "").split("+");
  let key = parts.pop();
  if (key === "" && parts.length) key = "+"; // "Shift++"
  const names = [...parts, ...(Array.isArray(step.modifiers) ? step.modifiers : [])].map((m) => String(m).toLowerCase());
  if (names.some((m) => !(m in MODIFIER_BITS))) return null;
  const modifiers = names.reduce((bits, m) => bits | MODIFIER_BITS[m], 0);
  let spec = NAMED_KEYS[key];
  let shown = key;
  if (spec) shown = spec.key ?? key;
  else if (/^[A-Za-z]$/.test(key)) {
    const upper = key.toUpperCase();
    shown = modifiers & MODIFIER_BITS.shift ? upper : key.toLowerCase();
    spec = { code: `Key${upper}`, vk: upper.charCodeAt(0), text: shown };
  } else if (/^[0-9]$/.test(key)) {
    spec = { code: `Digit${key}`, vk: key.charCodeAt(0), text: key };
  } else return null;
  const base = { key: shown, code: spec.code, windowsVirtualKeyCode: spec.vk, nativeVirtualKeyCode: spec.vk, modifiers };
  const typed = spec.text !== undefined && !(modifiers & (MODIFIER_BITS.ctrl | MODIFIER_BITS.meta | MODIFIER_BITS.alt));
  return [
    typed ? { type: "keyDown", ...base, text: spec.text } : { type: "rawKeyDown", ...base },
    { type: "keyUp", ...base },
  ];
}

const validBy = (by) =>
  (typeof by === "number" && Number.isFinite(by)) ||
  (by && typeof by === "object" && !Array.isArray(by) && Object.keys(by).length > 0 &&
    Object.entries(by).every(([k, v]) => (k === "x" || k === "y") && typeof v === "number" && Number.isFinite(v)));

function validate(steps, web) {
  steps.forEach((s, i) => {
    if (!s || typeof s !== "object" || !OPS.has(s.op)) refuse(`step ${i + 1}: unknown op`);
    if (s.op === "open" && web && !isAllowedUrl(s.url, web.allowlist)) refuse(`step ${i + 1}: ${String(s.url).slice(0, 80)} is outside the allowlist`);
    if (s.op === "open" && !web && !isLoopbackUrl(s.url)) refuse(`step ${i + 1}: ${String(s.url).slice(0, 80)} is not a loopback http URL`);
    if (s.op === "type" && s.secret !== undefined) {
      if (!web) refuse(`step ${i + 1}: a secret can be typed only in a web QA run`);
      if (!["username", "password"].includes(s.secret) || s.text !== undefined) refuse(`step ${i + 1}: type takes "secret":"username"|"password" or "text", not both`);
    }
    if (s.op === "screenshot" && !/^[A-Za-z0-9][A-Za-z0-9._-]{0,80}$/.test(String(s.name ?? ""))) {
      refuse(`step ${i + 1}: screenshot name must be letters, digits, dot, dash or underscore`);
    }
    if (s.op === "screenshot" && s.fullPage !== undefined && typeof s.fullPage !== "boolean") {
      refuse(`step ${i + 1}: fullPage must be true or false`);
    }
    if (s.op === "press" && !keyEvents(s)) refuse(`step ${i + 1}: press needs a known key and modifiers from alt, ctrl, meta, shift`);
    if (s.op === "scroll") {
      const given = ["selector", "text", "by"].filter((k) => s[k] !== undefined);
      if (given.length !== 1) refuse(`step ${i + 1}: scroll needs exactly one of selector, text or by`);
      if (s.by !== undefined && !validBy(s.by)) refuse(`step ${i + 1}: scroll "by" must be a number of pixels or {"x":n,"y":n}`);
      if ((s.selector !== undefined || s.text !== undefined) && !String(s.selector ?? s.text)) refuse(`step ${i + 1}: scroll target is empty`);
    }
    if (s.op === "select") {
      if (!s.selector || typeof s.selector !== "string") refuse(`step ${i + 1}: select needs a selector`);
      if ((s.label === undefined) === (s.value === undefined)) refuse(`step ${i + 1}: select needs exactly one of label or value`);
    }
  });
}

// The watchdog: a detached process that polls this driver's pid and kills the browser's group once it is gone.
const WATCHDOG = `
const [driver, group, dir] = process.argv.slice(1);
const alive = (pid) => { try { process.kill(Number(pid), 0); return true; } catch (e) { return e.code === "EPERM"; } };
const tick = setInterval(() => {
  if (alive(driver)) return;
  try { process.kill(-Number(group), "SIGKILL"); } catch {}
  try { require("node:fs").rmSync(dir, { recursive: true, force: true }); } catch {}
  clearInterval(tick);
  process.exit(0);
}, 1000);
`;

class Cdp {
  constructor(url) {
    this.ws = new WebSocket(url);
    this.nextId = 1;
    this.pending = new Map();
    this.onEvent = null;
    this.opened = new Promise((resolve, reject) => {
      this.ws.addEventListener("open", resolve);
      this.ws.addEventListener("error", () => reject(new Error("cannot connect to the browser")));
    });
    this.ws.addEventListener("message", (m) => {
      const msg = JSON.parse(String(m.data));
      if (msg.id && this.pending.has(msg.id)) {
        const { resolve, reject } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        msg.error ? reject(new Error(msg.error.message || "cdp error")) : resolve(msg.result ?? {});
      } else if (msg.method) this.onEvent?.(msg);
    });
  }
  send(method, params = {}, sessionId) {
    const id = this.nextId++;
    const payload = { id, method, params };
    if (sessionId) payload.sessionId = sessionId;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, 20000);
      this.pending.set(id, {
        resolve: (v) => (clearTimeout(timer), resolve(v)),
        reject: (e) => (clearTimeout(timer), reject(e)),
      });
      this.ws.send(JSON.stringify(payload));
    });
  }
  close() {
    try {
      this.ws.close();
    } catch {}
  }
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const web = webTarget();
  if (!allowedOut(args.out)) {
    refuse(web ? `--out must be <run>/webqa/shots in a web QA run: ${args.out}` :
      `--out must be <run>/reviews/qa (with DCV2_RUN_DIR set) or a dcv2-qa-* folder in the temp dir: ${args.out}`);
  }
  if (web) {
    // Known from the start, so nothing the page says before the first type step can print it either.
    try {
      const account = JSON.parse(fs.readFileSync(path.join(web.runDir, "webqa-secret.json"), "utf8"));
      for (const v of [account.username, account.password]) if (typeof v === "string" && v) SECRETS.add(v);
    } catch {}
  }
  const steps = await readSteps(args.steps);
  validate(steps, web);
  if (!steps.length) refuse("no steps read (no step lines printed)");
  const browser = findBrowser();
  if (!browser) refuse("no Chrome-family browser found", 3);
  fs.mkdirSync(args.out, { recursive: true });

  const profile = fs.mkdtempSync(path.join(os.tmpdir(), "dcv2-qa-profile-"));
  const child = spawn(
    browser,
    [
      "--headless=new",
      "--remote-debugging-port=0",
      `--user-data-dir=${profile}`,
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-gpu",
      "--disable-extensions",
      "--window-size=1280,800",
      // Nothing but loopback (or, in web mode, the allowlist hosts) can be reached, whatever the page asks for. A system proxy would
      // bypass the resolver rules for subresources, so web mode turns the proxy off.
      web
        ? `--host-resolver-rules=MAP * ~NOTFOUND, ${web.hosts.map((h) => `EXCLUDE ${h}`).join(", ")}`
        : "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE localhost, EXCLUDE 127.0.0.1",
      ...(web ? ["--no-proxy-server"] : []),
      "about:blank",
    ],
    { detached: true, stdio: ["ignore", "ignore", "pipe"] },
  );
  const group = child.pid;
  const watchdog = spawn(process.execPath, ["-e", WATCHDOG, String(process.pid), String(group), profile], {
    detached: true,
    stdio: "ignore",
  });
  watchdog.unref();

  let cleaned = false;
  const cleanup = () => {
    if (cleaned) return;
    cleaned = true;
    try {
      process.kill(-group, "SIGKILL");
    } catch {}
    try {
      process.kill(watchdog.pid, "SIGKILL");
    } catch {}
    try {
      fs.rmSync(profile, { recursive: true, force: true });
    } catch {}
  };
  process.on("exit", cleanup);
  for (const sig of ["SIGINT", "SIGTERM", "SIGHUP"]) process.on(sig, () => (cleanup(), process.exit(130)));

  const wsUrl = await new Promise((resolve) => {
    let buf = "";
    const timer = setTimeout(() => resolve(null), 20000);
    child.on("exit", () => resolve(null));
    child.stderr.on("data", (d) => {
      buf += d;
      const m = buf.match(/DevTools listening on (ws:\/\/\S+)/);
      if (m) (clearTimeout(timer), resolve(m[1]));
    });
  });
  if (!wsUrl) refuse("the browser did not offer a DevTools endpoint", 1);
  const u = new URL(wsUrl);
  if (!["127.0.0.1", "localhost", "[::1]"].includes(u.hostname)) refuse("the DevTools endpoint is not on loopback", 1);

  const cdp = new Cdp(wsUrl);
  await cdp.opened;
  const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
  const page = (method, params) => cdp.send(method, params, sessionId);
  let currentStep = 0;
  let left = null; // a main-frame navigation outside the allowlist, seen by Page.frameNavigated (web mode)
  const requestHost = new Map();
  const requests = new Map(); // requestId -> { url, type }: what a network page-error line names
  const NET_TYPES = new Set(["Document", "Script", "Stylesheet", "Image", "Fetch", "XHR"]);
  const netSeen = new Set(); // url + status already printed in the current step
  let netStep = -1;
  let pageOrigin = null; // origin of the last opened URL: the page's own origin
  const netError = (id, what) => {
    const r = requests.get(id);
    if (!r || !NET_TYPES.has(r.type)) return;
    // Web mode (real sites): only the page's own document. Outside it (the loopback app) every listed type prints.
    if (web) {
      let origin = null;
      try {
        origin = new URL(r.url).origin;
      } catch {}
      if (r.type !== "Document" || !pageOrigin || origin !== pageOrigin) return;
    }
    if (netStep !== currentStep) (netSeen.clear(), (netStep = currentStep));
    const key = `${r.url} ${what}`;
    if (netSeen.has(key) || netSeen.has(`log ${r.url}`)) return;
    netSeen.add(key);
    netSeen.add(`net ${r.url}`);
    out({ event: "page-error", kind: "network", message: `${what} ${r.type}`.slice(0, 300), url: r.url.slice(0, 300), step: currentStep });
  };
  const blocked = new Map(); // host -> failed requests: hosts the browser could not resolve because they are off the allowlist
  const pageError = (kind, message) => out({ event: "page-error", kind, message: String(message).slice(0, 300), step: currentStep });
  cdp.onEvent = (msg) => {
    if (msg.sessionId && msg.sessionId !== sessionId) return;
    const p = msg.params || {};
    if (web && msg.method === "Page.frameNavigated" && !p.frame?.parentId) {
      const url = p.frame?.url;
      if (url && url !== "about:blank" && !isAllowedUrl(url, web.allowlist)) {
        left = url;
        page("Page.navigate", { url: "about:blank" }).catch(() => {});
      }
    } else if (msg.method === "Network.requestWillBeSent") {
      requests.set(p.requestId, { url: String(p.request?.url ?? ""), type: p.type });
      if (requests.size > 2000) requests.delete(requests.keys().next().value);
      if (web) {
        try {
          requestHost.set(p.requestId, new URL(p.request?.url).hostname);
        } catch {}
      }
    } else if (msg.method === "Network.loadingFailed") {
      if (web && /ERR_NAME_NOT_RESOLVED/.test(p.errorText || "")) {
        const host = requestHost.get(p.requestId);
        if (host) blocked.set(host, (blocked.get(host) || 0) + 1);
      } else if (!p.canceled && !p.blockedReason) {
        netError(p.requestId, String(p.errorText || "load failed"));
      }
    } else if (msg.method === "Network.responseReceived") {
      const status = Number(p.response?.status);
      if (status >= 400) netError(p.requestId, `HTTP ${status}`);
    } else if (msg.method === "Runtime.exceptionThrown") {
      const d = p.exceptionDetails || {};
      pageError("exception", d.exception?.description || d.text || "uncaught exception");
    } else if (msg.method === "Runtime.consoleAPICalled" && p.type === "error") {
      pageError("console", (p.args || []).map((a) => a.value ?? a.description ?? a.type).join(" "));
    } else if (msg.method === "Log.entryAdded" && p.entry?.level === "error") {
      const url = typeof p.entry.url === "string" && p.entry.url ? p.entry.url.slice(0, 300) : null;
      if (netStep !== currentStep) (netSeen.clear(), (netStep = currentStep));
      // The same failure may also arrive as a network line (the request id names it): print it once.
      if (url && (netSeen.has(`net ${url}`) || netSeen.has(`log ${url}`))) return;
      if (url) netSeen.add(`log ${url}`);
      const text = String(p.entry.text || "log error").slice(0, 300);
      out(url ? { event: "page-error", kind: "console", message: text, url, step: currentStep } : { event: "page-error", kind: "console", message: text, step: currentStep });
    }
  };
  await page("Page.enable");
  await page("Runtime.enable");
  await page("Log.enable");
  await page("Network.enable");
  const evaluate = async (expression) => {
    const r = await page("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.text || "script error");
    return r.result?.value;
  };
  const assertLoopbackPage = async () => {
    const href = await evaluate("location.href");
    if (web) {
      if (left) {
        const url = left;
        left = null;
        throw new Error(`the page left the allowlist (${String(url).slice(0, 80)})`);
      }
      if (href && href !== "about:blank" && !isAllowedUrl(href, web.allowlist)) {
        await page("Page.navigate", { url: "about:blank" });
        throw new Error(`the page left the allowlist (${String(href).slice(0, 80)})`);
      }
      return;
    }
    if (href && href !== "about:blank" && !isLoopbackUrl(href)) {
      await page("Page.navigate", { url: "about:blank" });
      throw new Error(`the page left loopback (${String(href).slice(0, 60)})`);
    }
  };
  const center = (find) => `(() => { ${find} if (!el) return null; el.scrollIntoView({block:"center"}); const r = el.getBoundingClientRect(); return {x: r.left + r.width/2, y: r.top + r.height/2}; })()`;

  const deadline = Date.now() + args.timeout * Math.max(1, steps.length);
  let failed = 0;
  let printed = 0;
  for (let i = 0; i < steps.length; i++) {
    const s = steps[i];
    currentStep = i + 1;
    const t0 = Date.now();
    try {
      if (Date.now() > deadline) throw new Error("time limit reached");
      let extra = {};
      if (s.op === "open") {
        try {
          pageOrigin = new URL(s.url).origin;
        } catch {}
        await page("Page.navigate", { url: s.url });
        await sleep(300);
        for (let k = 0; k < 50; k++) {
          if ((await evaluate("document.readyState")) === "complete") break;
          await sleep(100);
        }
        await assertLoopbackPage();
        extra = { title: String((await evaluate("document.title")) ?? "").slice(0, 120) };
      } else if (s.op === "click") {
        const find = s.selector
          ? `const el = document.querySelector(${JSON.stringify(String(s.selector))});`
          : `const want = ${JSON.stringify(String(s.text ?? ""))}.toLowerCase(); const el = [...document.querySelectorAll("button,a,[role=button],[role=menuitem],[role=tab],[role=option],option,input[type=button],input[type=submit],summary,label")].find((e) => ((e.innerText || e.value || e.getAttribute("aria-label") || "") + "").toLowerCase().includes(want));`;
        const pos = await evaluate(center(find));
        if (!pos) throw new Error("nothing to click");
        for (const type of ["mousePressed", "mouseReleased"]) {
          await page("Input.dispatchMouseEvent", { type, x: pos.x, y: pos.y, button: "left", clickCount: 1 });
        }
        await sleep(200);
        await assertLoopbackPage();
      } else if (s.op === "type" && s.secret) {
        let account;
        try {
          account = JSON.parse(fs.readFileSync(path.join(web.runDir, "webqa-secret.json"), "utf8"));
        } catch {
          throw new Error("no test account for this pass");
        }
        const value = account && typeof account[s.secret] === "string" ? account[s.secret] : "";
        if (!value) throw new Error(`the test account has no ${s.secret}`);
        for (const v of [account.username, account.password]) if (typeof v === "string" && v) SECRETS.add(v);
        const target = s.selector ? JSON.stringify(String(s.selector)) : "null";
        const found = await evaluate(`(() => { const el = ${target} ? document.querySelector(${target}) : document.activeElement; if (!el) return "none"; ${
          s.secret === "password" ? `if (!(el.tagName === "INPUT" && (el.getAttribute("type") || "").toLowerCase() === "password")) return "not-password";` : ""
        } el.focus(); return "ok"; })()`);
        if (found === "none") throw new Error("nothing to type into");
        if (found === "not-password") throw new Error("the password is typed only into an input[type=password] field");
        await page("Input.insertText", { text: value });
      } else if (s.op === "type") {
        if (s.selector) {
          const ok = await evaluate(`(() => { const el = document.querySelector(${JSON.stringify(String(s.selector))}); if (!el) return false; el.focus(); return true; })()`);
          if (!ok) throw new Error("nothing to type into");
        }
        await page("Input.insertText", { text: String(s.text ?? "") });
      } else if (s.op === "press") {
        for (const event of keyEvents(s)) await page("Input.dispatchKeyEvent", event);
        await sleep(100);
      } else if (s.op === "scroll") {
        let ok;
        if (s.by !== undefined) {
          const by = typeof s.by === "number" ? { x: 0, y: s.by } : { x: s.by.x ?? 0, y: s.by.y ?? 0 };
          ok = await evaluate(`(() => { window.scrollBy(${by.x}, ${by.y}); return true; })()`);
        } else {
          const find = s.selector
            ? `const el = document.querySelector(${JSON.stringify(String(s.selector))});`
            : `const want = ${JSON.stringify(String(s.text))}.toLowerCase(); const el = [...document.querySelectorAll("body *")].filter((e) => e.children.length === 0 || e.matches("button,a,label,summary,h1,h2,h3,h4,p,li,td,th")).find((e) => ((e.innerText || "") + "").toLowerCase().includes(want));`;
          ok = await evaluate(`(() => { ${find} if (!el) return false; el.scrollIntoView({block: "center", inline: "nearest"}); return true; })()`);
        }
        if (!ok) throw new Error("nothing to scroll to");
        await sleep(100);
      } else if (s.op === "select") {
        const wanted = s.label !== undefined ? { by: "label", text: String(s.label) } : { by: "value", text: String(s.value) };
        const r = await evaluate(`(() => {
          const el = document.querySelector(${JSON.stringify(s.selector)});
          if (!el || el.tagName !== "SELECT") return { ok: false, error: "no-select" };
          const want = ${JSON.stringify(wanted.text)};
          const opt = [...el.options].find((o) => (${JSON.stringify(wanted.by)} === "label" ? (o.label || o.textContent || "").trim() : o.value) === want);
          if (!opt) return { ok: false, error: "no-option" };
          el.value = opt.value;
          el.dispatchEvent(new Event("input", { bubbles: true }));
          el.dispatchEvent(new Event("change", { bubbles: true }));
          return { ok: true };
        })()`);
        if (!r?.ok) {
          throw new Error(r?.error === "no-option" ? `no option ${wanted.by} "${wanted.text.slice(0, 60)}" in ${s.selector.slice(0, 60)}` : `no <select> matches ${s.selector.slice(0, 60)}`);
        }
        await sleep(100);
      } else if (s.op === "wait-text") {
        const want = String(s.text ?? "");
        const until = Date.now() + Number(s.timeoutMs || 5000);
        let seen = false;
        while (Date.now() < until) {
          seen = await evaluate(`document.body && document.body.innerText.includes(${JSON.stringify(want)})`);
          if (seen) break;
          await sleep(150);
        }
        if (!seen) throw new Error(`text not seen: ${want.slice(0, 60)}`);
      } else if (s.op === "text") {
        const value = s.selector
          ? await evaluate(`(document.querySelector(${JSON.stringify(String(s.selector))}) || {}).innerText ?? null`)
          : await evaluate("document.body ? document.body.innerText : ''");
        if (value === null) throw new Error("nothing matches the selector");
        extra = { text: String(value).slice(0, MAX_TEXT) };
      } else if (s.op === "screenshot") {
        let shot = { format: "png" };
        if (s.fullPage) {
          // The whole page: its content size as the clip, drawn beyond the viewport.
          const m = await page("Page.getLayoutMetrics");
          const size = m.cssContentSize || m.contentSize || {};
          const width = Math.ceil(size.width || 1280);
          const height = Math.min(Math.ceil(size.height || 800), 16000);
          shot = { format: "png", captureBeyondViewport: true, clip: { x: 0, y: 0, width, height, scale: 1 } };
        }
        const { data } = await page("Page.captureScreenshot", shot);
        const bytes = Buffer.from(data, "base64");
        if (bytes.length > MAX_PNG_BYTES) throw new Error("screenshot is over 5 MB");
        const file = path.join(args.out, `${s.name}.png`);
        if (path.dirname(path.resolve(file)) !== path.resolve(args.out)) throw new Error("bad screenshot name");
        try {
          if (fs.lstatSync(file).isSymbolicLink()) throw new Error("a link is in the way of the screenshot");
        } catch (e) {
          if (e.code !== "ENOENT") throw e;
        }
        fs.writeFileSync(file, bytes, { flag: "w", mode: 0o600 });
        extra = { path: file, bytes: bytes.length };
      }
      if (web && left) {
        const url = left;
        left = null;
        throw new Error(`the page left the allowlist (${String(url).slice(0, 80)})`);
      }
      printed++;
      out({ step: i + 1, op: s.op, ok: true, ms: Date.now() - t0, ...extra });
    } catch (e) {
      failed++;
      printed++;
      out({ step: i + 1, op: s.op, ok: false, error: String(e.message || e).slice(0, 200) });
    }
  }
  if (blocked.size) {
    const hosts = [...blocked.keys()].slice(0, 3).join(", ");
    pageError("console", `blocked hosts: ${blocked.size} (${hosts})`);
  }
  cdp.close();
  cleanup();
  if (!printed) refuse("no steps read (no step lines printed)");
  process.exit(failed ? 1 : 0);
}

// Whether this file is the program being run: real paths on both sides, so a start through a symlinked folder (macOS /var -> /private/var)
// still runs main() instead of exiting 0 with no output.
function isEntry() {
  if (!process.argv[1]) return false;
  const here = fileURLToPath(import.meta.url);
  try {
    return fs.realpathSync(here) === fs.realpathSync(process.argv[1]);
  } catch {
    return here === path.resolve(process.argv[1]);
  }
}

if (isEntry()) {
  main().catch((e) => {
    process.stderr.write(`qa-browser: ${e.message || e}\n`);
    process.exit(1);
  });
}
