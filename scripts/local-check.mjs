// Local stand-in for CI: `npm run check`.
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

// The gate for every change: `npm run check`. Rust first (format, lints,
// tests), then the page (unit tests, then tsc and the production build).
const steps = [
  {
    name: "cargo fmt",
    cmd: "cargo",
    args: ["fmt", "--manifest-path", "src-tauri/Cargo.toml", "--", "--check"],
  },
  {
    name: "cargo clippy",
    cmd: "cargo",
    args: [
      "clippy",
      "--manifest-path",
      "src-tauri/Cargo.toml",
      "--all-targets",
      "--",
      "-D",
      "warnings",
    ],
  },
  {
    name: "cargo test",
    cmd: "cargo",
    args: ["test", "--manifest-path", "src-tauri/Cargo.toml"],
  },
  { name: "npm test", cmd: "npm", args: ["test"] },
  // `tsc && vite build`: the type check, then the bundle.
  { name: "npm run build", cmd: "npm", args: ["run", "build"] },
];

function readJson(filePath) {
  return JSON.parse(readFileSync(filePath, "utf8"));
}

function cargoPackageVersion() {
  const text = readFileSync("src-tauri/Cargo.toml", "utf8");
  const match = text.match(/\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m);
  if (!match) {
    throw new Error("could not read version from src-tauri/Cargo.toml");
  }
  return match[1];
}

function assertReleaseVersionsMatch() {
  const npmVersion = readJson("package.json").version;
  const tauriVersion = readJson("src-tauri/tauri.conf.json").version;
  const cargoVersion = cargoPackageVersion();
  const versions = { npm: npmVersion, tauri: tauriVersion, cargo: cargoVersion };
  const unique = new Set(Object.values(versions));
  if (unique.size !== 1) {
    throw new Error(
      `release versions differ (${JSON.stringify(versions)}). ` +
        "Bump package.json, src-tauri/tauri.conf.json, and src-tauri/Cargo.toml together.",
    );
  }
  console.log(`release version ${npmVersion} matches across npm, Tauri, and Cargo`);
}

// Characters that must be quoted. `%` is handled separately: cmd /c does
// not turn `%%` back into `%` inside a quoted argument, so a percent is
// written as `^%` outside quotes.
const CMD_QUOTED = /[\s"&|<>^!()]/;

/**
 * Quote one argv element for cmd.exe only when it needs it.
 * Embedded quotes are doubled. A `%` is escaped as `^%` outside quotes so
 * `cmd /c` passes a single percent through to the child. Do not
 * backslash-escape: cmd does not treat `\"` as a quote.
 */
export function quoteCmdArg(value) {
  const text = String(value);
  if (text.length === 0) {
    return '""';
  }
  if (!CMD_QUOTED.test(text)) {
    return text.replaceAll("%", "^%");
  }
  let quoted = "";
  let open = false;
  const ensureOpen = () => {
    if (!open) {
      quoted += '"';
      open = true;
    }
  };
  const ensureClosed = () => {
    if (open) {
      quoted += '"';
      open = false;
    }
  };
  for (const char of text) {
    if (char === "%") {
      ensureClosed();
      quoted += "^%";
      continue;
    }
    ensureOpen();
    quoted += char === '"' ? '""' : char;
  }
  ensureClosed();
  return quoted;
}

/**
 * Bare command token. A quoted name with no path (`"npm"`) makes cmd set
 * `%~dp0` to the current directory, so npm.cmd looks for npm-cli.js under
 * the project instead of `C:\Program Files\nodejs`. Leave bare names
 * unquoted. A path may be quoted; `%~dp0` is correct when the name contains
 * a separator.
 */
export function quoteCmdCommand(command) {
  const text = String(command);
  const token = quoteCmdArg(text);
  const hasPath = /[\\/]/.test(text);
  if (!hasPath && token.startsWith('"')) {
    throw new Error(
      `refusing to quote bare command ${JSON.stringify(text)}; ` +
        "cmd %~dp0 breaks for a quoted name with no path",
    );
  }
  return token;
}

/**
 * Argument to `cmd.exe /d /s /c`. `/s` strips one leading quote and the
 * final quote on the line, so the payload keeps an extra outer pair.
 */
export function windowsCmdPayload(command, args) {
  const joined = [quoteCmdCommand(command), ...args.map(quoteCmdArg)].join(" ");
  return `"${joined}"`;
}

/**
 * Spawn args for Windows. `windowsVerbatimArguments` must stay true: Node
 * otherwise re-quotes the `/c` string with backslashes, which cmd ignores.
 */
export function windowsInvocation(command, args, comspec = process.env.ComSpec) {
  const shell = comspec && /\\cmd(?:\.exe)?$/i.test(comspec) ? comspec : "cmd.exe";
  return {
    file: shell,
    args: ["/d", "/s", "/c", windowsCmdPayload(command, args)],
    windowsVerbatimArguments: true,
  };
}

/**
 * npm and npx are `.cmd` shims. Spawning those without a shell throws
 * EINVAL (CVE-2024-27980). `.exe` targets, including `node.exe` and
 * `cargo.exe`, are spawned directly so cmd does not rewrite arguments.
 */
export function usesCmdShim(command) {
  const base = path.win32.basename(String(command)).toLowerCase();
  if (base.endsWith(".cmd") || base.endsWith(".bat")) {
    return true;
  }
  if (base.endsWith(".exe") || base.endsWith(".com")) {
    return false;
  }
  return base === "npm" || base === "npx";
}

/**
 * Run a command. Clippy's `-- -D warnings` stays separate arguments.
 * `options.env` is added to this process's environment.
 * On Windows only `.cmd`/`.bat` shims go through `cmd.exe /d /s /c`.
 */
export function spawnCommand(command, args, options = {}) {
  const stdio = options.stdio ?? "inherit";
  const env = options.env ? { ...process.env, ...options.env } : undefined;
  if (process.platform === "win32" && usesCmdShim(command)) {
    const invocation = windowsInvocation(command, args);
    return spawnSync(invocation.file, invocation.args, {
      stdio,
      windowsVerbatimArguments: invocation.windowsVerbatimArguments,
      encoding: options.encoding,
      env,
    });
  }
  return spawnSync(command, args, {
    stdio,
    encoding: options.encoding,
    env,
  });
}

function runStep(step) {
  console.log(`\n==> ${step.name}`);
  const result = spawnCommand(step.cmd, step.args, { env: step.env });
  if (result.error) {
    console.error(`\n${step.name} failed to start: ${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) {
    const detail = result.signal ? `signal ${result.signal}` : `exit ${result.status}`;
    console.error(`\n${step.name} failed (${detail})`);
    process.exit(result.status === null || result.status === undefined ? 1 : result.status);
  }
}

function isDirectRun() {
  const entry = process.argv[1];
  if (!entry) {
    return false;
  }
  return import.meta.url === pathToFileURL(path.resolve(entry)).href;
}

export { steps };

if (isDirectRun()) {
  try {
    assertReleaseVersionsMatch();
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  }

  for (const step of steps) {
    runStep(step);
  }

  console.log("\nAll local checks passed.");
}
