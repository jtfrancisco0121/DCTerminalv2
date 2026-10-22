import { describe, expect, it } from "vitest";
import process from "node:process";
import {
  quoteCmdArg,
  spawnCommand,
  steps,
  usesCmdShim,
  windowsCmdPayload,
  windowsInvocation,
} from "./local-check.mjs";

/**
 * Undo cmd.exe `/d /s /c` outer-quote stripping, then split argv.
 * `^` outside quotes escapes the next character, so `^%` is one `%`.
 * Adjacent quoted and unquoted pieces stay one argument.
 */
function argvFromCmdPayload(payload) {
  expect(payload.startsWith('"')).toBe(true);
  expect(payload.endsWith('"')).toBe(true);
  const inner = payload.slice(1, -1);
  const tokens = [];
  let index = 0;
  while (index < inner.length) {
    while (inner[index] === " ") {
      index += 1;
    }
    if (index >= inner.length) {
      break;
    }
    let token = "";
    let inQuotes = false;
    while (index < inner.length) {
      const char = inner[index];
      if (!inQuotes && char === " ") {
        break;
      }
      if (!inQuotes && char === "^") {
        token += inner[index + 1] ?? "";
        index += 2;
        continue;
      }
      if (char === '"') {
        if (inQuotes && inner[index + 1] === '"') {
          token += '"';
          index += 2;
          continue;
        }
        inQuotes = !inQuotes;
        index += 1;
        continue;
      }
      token += char;
      index += 1;
    }
    tokens.push(token);
  }
  return tokens;
}

describe("windows command quoting", () => {
  it("keeps clippy -D warnings as its own arguments", () => {
    const clippy = steps.find((step) => step.name === "cargo clippy");
    expect(clippy).toBeDefined();
    expect(clippy.args).toContain("-D");
    expect(clippy.args).toContain("warnings");
    expect(clippy.args.at(-3)).toBe("--");

    const payload = windowsCmdPayload(clippy.cmd, clippy.args);
    expect(argvFromCmdPayload(payload)).toEqual([clippy.cmd, ...clippy.args]);
  });

  it("quotes spaces, quotes, percent signs, and empty args", () => {
    const args = [
      "C:\\Program Files\\nodejs\\node.exe",
      "hello world",
      'say "hi"',
      "100%",
      "",
    ];
    expect(argvFromCmdPayload(windowsCmdPayload("npm", args))).toEqual(["npm", ...args]);
  });

  it("escapes percent signs with a caret outside quotes", () => {
    expect(quoteCmdArg("100%")).toBe("100^%");
    expect(quoteCmdArg('a"b%')).toBe('"a""b"^%');
    expect(quoteCmdArg("hello 100% world")).toBe('"hello 100"^%" world"');
    expect(quoteCmdArg("test")).toBe("test");
    expect(argvFromCmdPayload(windowsCmdPayload("npm", ["100%"]))).toEqual(["npm", "100%"]);
  });

  it("uses cmd only for batch shims", () => {
    expect(usesCmdShim("npm")).toBe(true);
    expect(usesCmdShim("npx")).toBe(true);
    expect(usesCmdShim("C:\\Program Files\\nodejs\\npm.cmd")).toBe(true);
    expect(usesCmdShim("C:\\Program Files\\nodejs\\node.exe")).toBe(false);
    expect(usesCmdShim("cargo")).toBe(false);
  });

  it("leaves the npm command unquoted so %~dp0 is the nodejs directory", () => {
    const npmTest = steps.find((step) => step.name === "npm test");
    expect(npmTest).toBeDefined();
    const payload = windowsCmdPayload(npmTest.cmd, npmTest.args);
    const invocation = windowsInvocation(npmTest.cmd, npmTest.args);
    // Outer quotes are only for cmd /s. The command itself is bare.
    expect(payload).toBe('"npm test"');
    expect(payload.includes('"npm"')).toBe(false);
    expect(payload.includes("\\")).toBe(false);
    expect(invocation.windowsVerbatimArguments).toBe(true);
    expect(invocation.args).toEqual(["/d", "/s", "/c", '"npm test"']);
    expect(argvFromCmdPayload(payload)).toEqual(["npm", "test"]);
  });

  it("quotes an absolute npm.cmd path that contains spaces", () => {
    const command = "C:\\Program Files\\nodejs\\npm.cmd";
    const payload = windowsCmdPayload(command, ["test"]);
    expect(payload).toBe(`""${command}" test"`);
    expect(argvFromCmdPayload(payload)).toEqual([command, "test"]);
  });
});

describe("spawnCommand", () => {
  it("returns the child exit code", () => {
    const result = spawnCommand(process.execPath, ["-e", "process.exit(3)"], {
      stdio: "pipe",
      encoding: "utf8",
    });
    expect(result.error).toBeUndefined();
    expect(result.status).toBe(3);
  });

  it("passes clippy-style arguments through to the child", () => {
    const args = ["--", "-D", "warnings", "hello world", 'say "hi"', "100%"];
    // The first `--` ends Node's own options so the child sees the rest.
    const result = spawnCommand(
      process.execPath,
      ["-e", "process.stdout.write(JSON.stringify(process.argv.slice(1)))", "--", ...args],
      { stdio: "pipe", encoding: "utf8" },
    );
    expect(result.status).toBe(0);
    expect(JSON.parse(result.stdout)).toEqual(args);
  });
});

describe("check steps", () => {
  it("runs fmt, clippy, cargo test, vitest and the build, in that order", () => {
    expect(steps.map((step) => step.name)).toEqual([
      "cargo fmt",
      "cargo clippy",
      "cargo test",
      "npm test",
      "npm run build",
    ]);
    const fmt = steps.find((step) => step.name === "cargo fmt");
    expect(fmt.args.slice(-2)).toEqual(["--", "--check"]);
  });

  it("adds a step's env to the child's environment", () => {
    const result = spawnCommand(
      process.execPath,
      ["-e", "process.stdout.write(`${process.env.DCTV2_CHECK_PROBE}|${!!process.env.PATH}`)"],
      { stdio: "pipe", encoding: "utf8", env: { DCTV2_CHECK_PROBE: "1" } },
    );
    expect(result.status).toBe(0);
    expect(result.stdout).toBe("1|true");
  });
});
