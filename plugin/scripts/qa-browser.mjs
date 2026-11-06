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
//   {"op":"press","key":"Enter"}
//   {"op":"wait-text","text":"Planning","timeoutMs":5000}
//   {"op":"text","selector":"main"}                    (selector optional: the whole page)
//   {"op":"screenshot","name":"S1.1"}                  (writes <out>/S1.1.png)
//
// Exit: 0 all steps ok, 1 a step failed, 2 refused (non-loopback URL, bad out folder, bad step), 3 no Chrome-family
// browser found. The browser runs in its own process group; it is killed on exit, on signals, and by a detached
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

function out(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}
function refuse(message, code = 2) {
  process.stderr.write(`qa-browser: ${message}\n`);
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

const OPS = new Set(["open", "click", "type", "press", "wait-text", "text", "screenshot"]);

function validate(steps) {
  steps.forEach((s, i) => {
    if (!s || typeof s !== "object" || !OPS.has(s.op)) refuse(`step ${i + 1}: unknown op`);
    if (s.op === "open" && !isLoopbackUrl(s.url)) refuse(`step ${i + 1}: ${String(s.url).slice(0, 80)} is not a loopback http URL`);
    if (s.op === "screenshot" && !/^[A-Za-z0-9][A-Za-z0-9._-]{0,80}$/.test(String(s.name ?? ""))) {
      refuse(`step ${i + 1}: screenshot name must be letters, digits, dot, dash or underscore`);
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
    this.events = [];
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
      } else if (msg.method) this.events.push(msg);
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
  if (!allowedOut(args.out)) {
    refuse(`--out must be <run>/reviews/qa (with DCV2_RUN_DIR set) or a dcv2-qa-* folder in the temp dir: ${args.out}`);
  }
  const steps = await readSteps(args.steps);
  validate(steps);
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
      // Nothing but loopback can be reached, whatever the page asks for.
      "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE localhost, EXCLUDE 127.0.0.1",
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
  await page("Page.enable");
  await page("Runtime.enable");
  const evaluate = async (expression) => {
    const r = await page("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.text || "script error");
    return r.result?.value;
  };
  const assertLoopbackPage = async () => {
    const href = await evaluate("location.href");
    if (href && href !== "about:blank" && !isLoopbackUrl(href)) {
      await page("Page.navigate", { url: "about:blank" });
      throw new Error(`the page left loopback (${String(href).slice(0, 60)})`);
    }
  };
  const center = (find) => `(() => { ${find} if (!el) return null; el.scrollIntoView({block:"center"}); const r = el.getBoundingClientRect(); return {x: r.left + r.width/2, y: r.top + r.height/2}; })()`;

  const deadline = Date.now() + args.timeout * Math.max(1, steps.length);
  let failed = 0;
  for (let i = 0; i < steps.length; i++) {
    const s = steps[i];
    const t0 = Date.now();
    try {
      if (Date.now() > deadline) throw new Error("time limit reached");
      let extra = {};
      if (s.op === "open") {
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
          : `const want = ${JSON.stringify(String(s.text ?? ""))}.toLowerCase(); const el = [...document.querySelectorAll("button,a,[role=button],input[type=button],input[type=submit],summary,label")].find((e) => ((e.innerText || e.value || e.getAttribute("aria-label") || "") + "").toLowerCase().includes(want));`;
        const pos = await evaluate(center(find));
        if (!pos) throw new Error("nothing to click");
        for (const type of ["mousePressed", "mouseReleased"]) {
          await page("Input.dispatchMouseEvent", { type, x: pos.x, y: pos.y, button: "left", clickCount: 1 });
        }
        await sleep(200);
        await assertLoopbackPage();
      } else if (s.op === "type") {
        if (s.selector) {
          const ok = await evaluate(`(() => { const el = document.querySelector(${JSON.stringify(String(s.selector))}); if (!el) return false; el.focus(); return true; })()`);
          if (!ok) throw new Error("nothing to type into");
        }
        await page("Input.insertText", { text: String(s.text ?? "") });
      } else if (s.op === "press") {
        const key = String(s.key ?? "");
        await page("Input.dispatchKeyEvent", { type: "keyDown", key });
        await page("Input.dispatchKeyEvent", { type: "keyUp", key });
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
        const { data } = await page("Page.captureScreenshot", { format: "png" });
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
      out({ step: i + 1, op: s.op, ok: true, ms: Date.now() - t0, ...extra });
    } catch (e) {
      failed++;
      out({ step: i + 1, op: s.op, ok: false, error: String(e.message || e).slice(0, 200) });
    }
  }
  cdp.close();
  cleanup();
  process.exit(failed ? 1 : 0);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  main().catch((e) => {
    process.stderr.write(`qa-browser: ${e.message || e}\n`);
    process.exit(1);
  });
}
