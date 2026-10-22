/**
 * Tauri `beforeDevCommand`: start Vite if :1420 is free, or reuse an existing server.
 * Waits until the port accepts connections before exiting (reuse path).
 */
import { spawn } from "node:child_process";
import net from "node:net";

const PORT = 1420;
const HOST = "127.0.0.1";

function isPortOpen(port) {
  return new Promise((resolve) => {
    const socket = net.createConnection({ port, host: HOST });
    socket.setTimeout(500);
    socket.on("connect", () => {
      socket.destroy();
      resolve(true);
    });
    socket.on("timeout", () => {
      socket.destroy();
      resolve(false);
    });
    socket.on("error", () => resolve(false));
  });
}

async function waitForPort(maxMs = 30_000) {
  const start = Date.now();
  while (Date.now() - start < maxMs) {
    if (await isPortOpen(PORT)) return true;
    await new Promise((r) => setTimeout(r, 200));
  }
  return false;
}

if (await isPortOpen(PORT)) {
  console.log(`[dev] Vite already on http://${HOST}:${PORT} — reusing`);
  process.exit(0);
}

console.log(`[dev] Starting Vite on http://${HOST}:${PORT}`);
const child = spawn("npm run dev", {
  stdio: "inherit",
  shell: true,
  env: process.env,
});

const ready = await waitForPort();
if (!ready) {
  console.error(`[dev] Timed out waiting for Vite on port ${PORT}`);
  child.kill();
  process.exit(1);
}

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 0);
});
