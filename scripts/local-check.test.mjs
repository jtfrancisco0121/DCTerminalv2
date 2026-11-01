import { describe, expect, it } from "vitest";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import { GIT_BASH_NOT_FOUND, resolveGitBash } from "./git-bash.mjs";
import {
  assertNoStrayHexColours,
  CROSS_CLIPPY,
  CROSS_CLIPPY_SKIP,
  envWithout,
  PLUGIN_SUITES,
  quoteCmdArg,
  spawnCommand,
  steps,
  stepsFor,
  strayHexColours,
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
  it("runs fmt, clippy, cargo test, vitest and the build, in that order, then the plugin suites", () => {
    const base = ["cargo fmt", "cargo clippy", "cargo test", "npm test", "npm run build"];
    const suites = ["guard", "state", "memory", "github", "stop_check", "plugin"].map(
      (name) => `plugin suite ${name}`,
    );
    expect(stepsFor("darwin").map((step) => step.name)).toEqual([...base, ...suites]);
    expect(stepsFor("linux").map((step) => step.name)).toEqual([...base, ...suites]);
    expect(steps.map((step) => step.name)).toEqual(stepsFor(process.platform).map((step) => step.name));
    const fmt = steps.find((step) => step.name === "cargo fmt");
    expect(fmt.args.slice(-2)).toEqual(["--", "--check"]);
  });

  it("runs the six hermetic bash suites and no live one", () => {
    expect(PLUGIN_SUITES).toEqual(["guard", "state", "memory", "github", "stop_check", "plugin"]);
    const scripts = stepsFor("darwin")
      .filter((step) => step.cmd === "bash")
      .map((step) => step.args.join(" "));
    expect(scripts).toEqual(PLUGIN_SUITES.map((name) => `scripts/skill-e2e/${name}_test.sh`));
    for (const live of ["run.sh", "golden.sh", "standalone.sh", "bmad_spike.sh"]) {
      expect(stepsFor("darwin").some((step) => step.args.join(" ").includes(live))).toBe(false);
    }
  });

  it("runs every plugin suite on Windows, under the Git Bash it was given", () => {
    const bash = "C:\\Program Files\\Git\\bin\\bash.exe";
    const win = stepsFor("win32", { bash });
    const suites = win.filter((step) => step.name.startsWith("plugin suite"));
    expect(suites.map((step) => step.name)).toEqual(PLUGIN_SUITES.map((name) => `plugin suite ${name}`));
    for (const step of suites) {
      expect(step.cmd).toBe(bash);
      expect(step.removeEnvPrefix).toBe("DCV2_");
    }
    expect(win.map((step) => step.name)).toContain("npm run build");
    expect(win.some((step) => step.name === CROSS_CLIPPY.name)).toBe(false);
  });

  it("adds the Windows cross-clippy step on darwin only when mingw is there, else a visible skip", () => {
    const base = stepsFor("darwin").map((step) => step.name);
    const withMingw = stepsFor("darwin", { mingw: true });
    expect(withMingw.map((step) => step.name)).toEqual([
      "cargo fmt",
      "cargo clippy",
      CROSS_CLIPPY.name,
      ...base.slice(2),
    ]);
    const cross = withMingw.find((step) => step.name === CROSS_CLIPPY.name);
    expect(cross.args).toContain("--target");
    expect(cross.args).toContain("x86_64-pc-windows-gnu");
    expect(cross.args.slice(-3)).toEqual(["--", "-D", "warnings"]);
    const without = stepsFor("darwin", { mingw: false });
    expect(without.filter((step) => step.note).map((step) => step.name)).toEqual([CROSS_CLIPPY_SKIP]);
    expect(without.filter((step) => !step.note).map((step) => step.name)).toEqual(base);
    expect(CROSS_CLIPPY_SKIP).toBe("skip: Windows cross-clippy (brew install mingw-w64)");
    expect(stepsFor("linux", { mingw: true }).map((step) => step.name)).toEqual(base);
  });

  it("gives the suites an environment with no DCV2_ variable", () => {
    expect(envWithout("DCV2_", { DCV2_RUN_DIR: "/x", DCV2_TAB_ID: "t", PATH: "/bin", DCTV2_KEEP: "1" })).toEqual({
      PATH: "/bin",
      DCTV2_KEEP: "1",
    });
    for (const step of stepsFor("darwin").filter((step) => step.cmd === "bash")) {
      expect(step.removeEnvPrefix).toBe("DCV2_");
    }
    process.env.DCV2_CHECK_LEAK_PROBE = "leaked";
    try {
      const result = spawnCommand(
        process.execPath,
        ["-e", "process.stdout.write(`${process.env.DCV2_CHECK_LEAK_PROBE}|${!!process.env.PATH}`)"],
        { stdio: "pipe", encoding: "utf8", removeEnvPrefix: "DCV2_" },
      );
      expect(result.stdout).toBe("undefined|true");
    } finally {
      delete process.env.DCV2_CHECK_LEAK_PROBE;
    }
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

describe("strayHexColours", () => {
  it("allows hex colours in the theme blocks", () => {
    const css = ":root {\n  --bg: #0d1117;\n}\n:root[data-theme=\"nord\"] {\n  --bg: #2e3440;\n}\n";
    expect(strayHexColours(css)).toEqual([]);
  });

  it("refuses a hex in a plain rule, in a var() fallback and after a theme block, with the line", () => {
    const css = [
      ":root {",
      "  --bg: #0d1117;",
      "}",
      ".x {",
      "  color: #fff;",
      "  border: 1px solid var(--a, #abcdef12);",
      "}",
      ":root[data-theme=\"nord\"] {",
      "  --bg: #2e3440;",
      "}",
      ".y { background: #123; }",
    ].join("\n");
    expect(strayHexColours(css)).toEqual([
      { line: 5, text: "color: #fff;" },
      { line: 6, text: "border: 1px solid var(--a, #abcdef12);" },
      { line: 11, text: ".y { background: #123; }" },
    ]);
  });

  it("ignores comments and id selectors", () => {
    const css = [
      "/* old: #ff0000 */",
      "#root { margin: 0; }",
      "#terminal-park,",
      "#add:hover,",
      "#fade { opacity: 1; }",
      ".z { /* #abc */ color: red; }",
    ].join("\n");
    expect(strayHexColours(css)).toEqual([]);
  });

  it("finds a hex in a nested at-rule but not a theme block inside one", () => {
    const css = "@media (min-width: 1px) {\n  .a {\n    color: #fff;\n  }\n}\n";
    expect(strayHexColours(css)).toEqual([{ line: 3, text: "color: #fff;" }]);
  });

  it("reports nothing for the real App.css", () => {
    expect(strayHexColours(readFileSync("src/App.css", "utf8"))).toEqual([]);
    expect(() => assertNoStrayHexColours()).not.toThrow();
  });

  it("throws naming the file and line", () => {
    const dir = mkdtempSync(path.join(tmpdir(), "stray-"));
    const file = path.join(dir, "App.css");
    writeFileSync(file, ".a {\n  color: #fff;\n}\n");
    expect(() => assertNoStrayHexColours(file)).toThrow(/App\.css:2: color: #fff;/);
  });
});

describe("Git Bash resolver (AD-2)", () => {
  const fixture = JSON.parse(
    readFileSync(path.join(import.meta.dirname, "..", "fixtures", "platform", "git-bash-cases.json"), "utf8"),
  );

  it("agrees with the shared fixture on every case", () => {
    expect(fixture.cases.length).toBeGreaterThanOrEqual(10);
    for (const c of fixture.cases) {
      const run = () =>
        resolveGitBash(
          (name) => c.env[name],
          (file) => c.files.includes(file),
          () => c.gitExecPath,
        );
      if (c.expect === null) {
        expect(run, c.name).toThrow(GIT_BASH_NOT_FOUND);
      } else {
        expect(run(), c.name).toBe(c.expect);
      }
    }
  });
});
