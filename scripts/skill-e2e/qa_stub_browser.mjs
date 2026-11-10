#!/usr/bin/env node
// A stand-in "browser" for qa_browser_test.sh: prints the DevTools line on stderr and answers the few CDP calls
// plugin/scripts/qa-browser.mjs makes over a minimal WebSocket server. No real browser, no network.
// STUB_PID_FILE: where it writes its pid. STUB_BIG=1: screenshots over 5 MB. STUB_LOG: a file that gets one JSON line
// per Input.dispatchKeyEvent, Page.captureScreenshot and scroll/select Runtime.evaluate. STUB_NO_SCROLL=1: a scroll
// target is not found. STUB_PAGE_ERRORS=1: after the page title is read, it sends an exception, a console.error, a
// log error and a console.log (which must not be reported). STUB_LEAK=1: the page text and the exception carry the secret
// "s3cr3t-pw". STUB_REDIRECT=<url>: every navigation ends there. STUB_NAV_AWAY=<url>: a main-frame navigation event to <url>
// arrives while a page loads. STUB_NOT_PASSWORD=1: the field being typed into is not a password field. STUB_BLOCKED=1: two
// requests to cdn.other.example fail with ERR_NAME_NOT_RESOLVED while the page loads.
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";

const logCall = (method, params) => {
  if (process.env.STUB_LOG) fs.appendFileSync(process.env.STUB_LOG, JSON.stringify({ method, params }) + "\n");
};

if (process.env.STUB_PID_FILE) fs.writeFileSync(process.env.STUB_PID_FILE, String(process.pid));
// STUB_ARGV_FILE: where it writes the browser flags it was started with, one per line.
if (process.env.STUB_ARGV_FILE) fs.writeFileSync(process.env.STUB_ARGV_FILE, process.argv.slice(2).join("\n") + "\n");

let url = "about:blank";
let sentErrors = false;
const PNG = Buffer.from("89504e470d0a1a0a0000000d49484452", "hex");

function answer(method, params) {
  switch (method) {
    case "Target.createTarget":
      return { targetId: "T1" };
    case "Target.attachToTarget":
      return { sessionId: "S1" };
    case "Page.navigate":
      url = process.env.STUB_REDIRECT && params.url !== "about:blank" ? process.env.STUB_REDIRECT : params.url;
      return {};
    case "Input.insertText":
      logCall(method, params);
      return {};
    case "Page.getLayoutMetrics":
      return { cssContentSize: { width: 1280, height: 2400 }, contentSize: { width: 1280, height: 2400 } };
    case "Input.dispatchKeyEvent":
      logCall(method, params);
      return {};
    case "Page.captureScreenshot": {
      logCall(method, params);
      const body = process.env.STUB_BIG ? Buffer.concat([PNG, Buffer.alloc(5 * 1024 * 1024 + 10)]) : PNG;
      return { data: body.toString("base64") };
    }
    case "Runtime.evaluate": {
      const e = String(params.expression);
      let value = null;
      if (e === "location.href") value = url;
      else if (e.includes('getAttribute("type")')) value = process.env.STUB_NOT_PASSWORD ? "not-password" : "ok";
      else if (e === "document.readyState") value = "complete";
      else if (e === "document.title") value = "Stub page";
      else if (e.includes("dispatchEvent")) {
        logCall("select", { expression: e });
        value = e.includes("Missing Option") ? { ok: false, error: "no-option" } : { ok: true };
      } else if (e.includes("scrollBy")) {
        logCall("scrollBy", { expression: e });
        value = true;
      } else if (e.includes("scrollIntoView") && !e.includes("getBoundingClientRect")) {
        logCall("scrollIntoView", { expression: e });
        value = !process.env.STUB_NO_SCROLL;
      } else if (e.includes("includes(")) value = e.includes("Never shown") ? false : true;
      else if (e.includes("innerText")) value = process.env.STUB_LEAK ? "Stub text s3cr3t-pw" : "Stub text";
      else value = { x: 10, y: 10 };
      return { result: { type: "object", value } };
    }
    default:
      return {};
  }
}

function frame(text) {
  const payload = Buffer.from(text);
  let head;
  if (payload.length < 126) head = Buffer.from([0x81, payload.length]);
  else if (payload.length < 65536) head = Buffer.from([0x81, 126, payload.length >> 8, payload.length & 255]);
  else {
    head = Buffer.alloc(10);
    head[0] = 0x81;
    head[1] = 127;
    head.writeBigUInt64BE(BigInt(payload.length), 2);
  }
  return Buffer.concat([head, payload]);
}

const server = http.createServer((_req, res) => res.end("stub"));
server.on("upgrade", (req, socket) => {
  const key = req.headers["sec-websocket-key"];
  const accept = crypto.createHash("sha1").update(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest("base64");
  socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
  let buf = Buffer.alloc(0);
  socket.on("data", (chunk) => {
    buf = Buffer.concat([buf, chunk]);
    for (;;) {
      if (buf.length < 2) return;
      const opcode = buf[0] & 15;
      let len = buf[1] & 127;
      let off = 2;
      if (len === 126) (len = buf.readUInt16BE(2)), (off = 4);
      else if (len === 127) (len = Number(buf.readBigUInt64BE(2))), (off = 10);
      if (buf.length < off + 4 + len) return;
      const mask = buf.subarray(off, off + 4);
      const data = Buffer.from(buf.subarray(off + 4, off + 4 + len));
      for (let i = 0; i < data.length; i++) data[i] ^= mask[i % 4];
      buf = buf.subarray(off + 4 + len);
      if (opcode === 8) return socket.end();
      if (opcode !== 1) continue;
      const msg = JSON.parse(data.toString());
      const reply = { id: msg.id, result: answer(msg.method, msg.params || {}) };
      if (msg.sessionId) reply.sessionId = msg.sessionId;
      const isTitle = msg.method === "Runtime.evaluate" && String(msg.params?.expression) === "document.title";
      const isReady = msg.method === "Runtime.evaluate" && String(msg.params?.expression) === "document.readyState";
      const send = (method, params) => socket.write(frame(JSON.stringify({ method, params, sessionId: msg.sessionId })));
      if (isReady && process.env.STUB_NAV_AWAY) send("Page.frameNavigated", { frame: { id: "F1", url: process.env.STUB_NAV_AWAY } });
      if (isTitle && process.env.STUB_BLOCKED) {
        for (const id of ["r1", "r2"]) {
          send("Network.requestWillBeSent", { requestId: id, request: { url: "https://cdn.other.example/lib.js" } });
          send("Network.loadingFailed", { requestId: id, errorText: "net::ERR_NAME_NOT_RESOLVED" });
        }
      }
      // The page's errors come before the answer of the title read, so they belong to the open step.
      if (process.env.STUB_PAGE_ERRORS && !sentErrors && isTitle) {
        sentErrors = true;
        const event = (method, params) => socket.write(frame(JSON.stringify({ method, params, sessionId: msg.sessionId })));
        event("Runtime.exceptionThrown", { exceptionDetails: { text: "Uncaught", exception: { description: "Error: boom at app.js:1" + (process.env.STUB_LEAK ? " pw=s3cr3t-pw" : "") } } });
        event("Runtime.consoleAPICalled", { type: "error", args: [{ type: "string", value: "request failed" }, { type: "number", value: 500 }] });
        event("Runtime.consoleAPICalled", { type: "log", args: [{ type: "string", value: "just a log" }] });
        event("Log.entryAdded", { entry: { level: "error", text: "net::ERR_FAILED" } });
        event("Log.entryAdded", { entry: { level: "info", text: "fine" } });
      }
      socket.write(frame(JSON.stringify(reply)));
    }
  });
  socket.on("error", () => {});
});
server.listen(0, "127.0.0.1", () => {
  process.stderr.write(`DevTools listening on ws://127.0.0.1:${server.address().port}/devtools/browser/stub\n`);
});
