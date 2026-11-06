#!/usr/bin/env node
// A stand-in "browser" for qa_browser_test.sh: prints the DevTools line on stderr and answers the few CDP calls
// plugin/scripts/qa-browser.mjs makes over a minimal WebSocket server. No real browser, no network.
// STUB_PID_FILE: where it writes its pid. STUB_BIG=1: screenshots over 5 MB.
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";

if (process.env.STUB_PID_FILE) fs.writeFileSync(process.env.STUB_PID_FILE, String(process.pid));

let url = "about:blank";
const PNG = Buffer.from("89504e470d0a1a0a0000000d49484452", "hex");

function answer(method, params) {
  switch (method) {
    case "Target.createTarget":
      return { targetId: "T1" };
    case "Target.attachToTarget":
      return { sessionId: "S1" };
    case "Page.navigate":
      url = params.url;
      return {};
    case "Page.captureScreenshot": {
      const body = process.env.STUB_BIG ? Buffer.concat([PNG, Buffer.alloc(5 * 1024 * 1024 + 10)]) : PNG;
      return { data: body.toString("base64") };
    }
    case "Runtime.evaluate": {
      const e = String(params.expression);
      let value = null;
      if (e === "location.href") value = url;
      else if (e === "document.readyState") value = "complete";
      else if (e === "document.title") value = "Stub page";
      else if (e.includes("includes(")) value = e.includes("Never shown") ? false : true;
      else if (e.includes("innerText")) value = "Stub text";
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
      socket.write(frame(JSON.stringify(reply)));
    }
  });
  socket.on("error", () => {});
});
server.listen(0, "127.0.0.1", () => {
  process.stderr.write(`DevTools listening on ws://127.0.0.1:${server.address().port}/devtools/browser/stub\n`);
});
