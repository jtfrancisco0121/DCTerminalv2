// Local stand-in for CI: `npm run check`.
import { existsSync, readFileSync, statSync } from "node:fs";
import { spawnSync } from "node:child_process";
import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";
import { resolveGitBash } from "./git-bash.mjs";

// The gate for every change: `npm run check`. Rust first (format, lints,
// tests), then the page (unit tests, then tsc and the production build), then
// the plugin's hermetic bash suites. There is no hosted CI: this is the gate.
const baseSteps = [
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

// Hermetic suites (each says "no Claude call"): stdlib bash and python only.
// The live suites (run.sh, golden.sh, standalone.sh, bmad_spike.sh) call
// Claude and are never part of the gate.
export const PLUGIN_SUITES = ["guard", "state", "memory", "github", "stop_check", "plugin"];

// A developer's own DCV2_* variables (a run's folder, a hook token, ...) must
// not leak into the suites: they would pass or fail by the machine they run on.
export const SUITE_ENV_PREFIX = "DCV2_";

/** The plugin suites, run by `bash` (on Windows: the Git Bash found by [`resolveGitBash`]). */
function pluginStepsWith(bash) {
  return PLUGIN_SUITES.map((name) => ({
    name: `plugin suite ${name}`,
    cmd: bash,
    args: [`scripts/skill-e2e/${name}_test.sh`],
    removeEnvPrefix: SUITE_ENV_PREFIX,
  }));
}

/** The Windows cross-compile lint: compiles every `cfg(windows)` branch on a Mac. */
export const CROSS_CLIPPY = {
  name: "cargo clippy (windows target)",
  cmd: "cargo",
  args: [
    "clippy",
    "--manifest-path",
    "src-tauri/Cargo.toml",
    "--target",
    "x86_64-pc-windows-gnu",
    "--all-targets",
    "--",
    "-D",
    "warnings",
  ],
};

export const CROSS_CLIPPY_SKIP = "skip: Windows cross-clippy (brew install mingw-w64)";

/** Whether `name` is an executable on the PATH. */
export function onPath(name, env = process.env) {
  const dirs = (env.PATH ?? "").split(path.delimiter).filter(Boolean);
  return dirs.some((dir) => existsSync(path.join(dir, name)));
}

/**
 * The gate's steps on `platform`. `opts.mingw` (darwin): true adds the Windows
 * cross-clippy step, false adds a visible skip note, undefined adds neither.
 * `opts.bash` (win32): the Git Bash that runs the plugin suites.
 */
export function stepsFor(platform, opts = {}) {
  if (platform === "win32") return [...baseSteps, ...pluginStepsWith(opts.bash ?? "bash")];
  const cross =
    platform === "darwin" && opts.mingw === true
      ? [CROSS_CLIPPY]
      : platform === "darwin" && opts.mingw === false
        ? [{ name: CROSS_CLIPPY_SKIP, note: true }]
        : [];
  // The cross-clippy step follows the host clippy: same lints, a second target.
  const [fmt, clippy, ...rest] = baseSteps;
  return [fmt, clippy, ...cross, ...rest, ...pluginStepsWith("bash")];
}

/** The real inputs of the Git Bash resolver on this machine. */
export function resolveGitBashHere() {
  return resolveGitBash(
    (name) => process.env[name],
    (file) => {
      try {
        return statSync(file).isFile();
      } catch {
        return false;
      }
    },
    () => {
      const out = spawnSync("git", ["--exec-path"], { encoding: "utf8" });
      return out.status === 0 ? out.stdout.trim() : null;
    },
  );
}

/** The steps with no machine-specific input (tests look steps up by name here). */
export const steps = stepsFor(process.platform);

function stepsHere() {
  if (process.platform === "win32") return stepsFor("win32", { bash: resolveGitBashHere() });
  return stepsFor(process.platform, { mingw: onPath("x86_64-w64-mingw32-gcc") });
}

/** `source` without the variables that start with `prefix`. */
export function envWithout(prefix, source = process.env) {
  return Object.fromEntries(Object.entries(source).filter(([key]) => !key.startsWith(prefix)));
}

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

const THEME_SELECTOR = /^:root(?:\[data-theme="[a-z0-9-]+"\])?$/;
const HEX_COLOUR = /#(?:[0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})\b/g;

/**
 * Hex colours in declaration values outside the theme blocks (`:root` and
 * `:root[data-theme="…"]`), as `{line, text}`. Colours belong to the themes;
 * everything else uses their tokens (a hex in a `var()` fallback counts).
 * Comments are ignored and selectors such as `#root` are not declarations.
 */
export function strayHexColours(css) {
  const clean = css.replace(/\/\*[\s\S]*?\*\//g, (comment) => comment.replace(/[^\n]/g, " "));
  const lines = css.split("\n");
  const found = [];
  const stack = [];
  let buffer = "";
  let bufferLine = 1;
  let line = 1;
  const skipping = () => stack.some(Boolean);
  const declaration = () => {
    const colon = buffer.indexOf(":");
    if (colon >= 0 && !skipping()) {
      const head = buffer.slice(0, colon);
      const before = (head.trimStart().match(/\n/g) ?? []).length;
      const value = buffer.slice(colon + 1);
      for (const match of value.matchAll(HEX_COLOUR)) {
        const at = bufferLine + before + (value.slice(0, match.index).match(/\n/g) ?? []).length;
        found.push({ line: at, text: (lines[at - 1] ?? "").trim() });
      }
    }
    buffer = "";
  };
  for (const char of clean) {
    if (char === "{") {
      stack.push(THEME_SELECTOR.test(buffer.trim()));
      buffer = "";
    } else if (char === ";") {
      declaration();
    } else if (char === "}") {
      declaration();
      stack.pop();
    } else {
      if (buffer.trim() === "" && char.trim() !== "") bufferLine = line;
      buffer += char;
    }
    if (char === "\n") line += 1;
  }
  return found;
}

export function assertNoStrayHexColours(file = "src/App.css") {
  const stray = strayHexColours(readFileSync(file, "utf8"));
  if (stray.length > 0) {
    const lines = stray.map(({ line, text }) => `App.css:${line}: ${text}`).join("\n");
    throw new Error(
      `hex colours outside the theme blocks in ${file}; use the theme tokens (var(--x)):\n${lines}`,
    );
  }
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
 * `options.env` is added to this process's environment;
 * `options.removeEnvPrefix` first drops this process's variables with that prefix.
 * On Windows only `.cmd`/`.bat` shims go through `cmd.exe /d /s /c`.
 */
export function spawnCommand(command, args, options = {}) {
  const stdio = options.stdio ?? "inherit";
  const base = options.removeEnvPrefix ? envWithout(options.removeEnvPrefix) : process.env;
  const env = options.env || options.removeEnvPrefix ? { ...base, ...options.env } : undefined;
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
  if (step.note) {
    console.log(`\n${step.name}`);
    return;
  }
  console.log(`\n==> ${step.name}`);
  const result = spawnCommand(step.cmd, step.args, {
    env: step.env,
    removeEnvPrefix: step.removeEnvPrefix,
  });
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

if (isDirectRun()) {
  try {
    assertReleaseVersionsMatch();
    assertNoStrayHexColours();
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  }

  let steps;
  try {
    steps = stepsHere();
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  }
  for (const step of steps) {
    runStep(step);
  }

  console.log("\nAll local checks passed.");
}
