import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import path from "node:path";
import { checkInstallerScript, MISSING_MESSAGE } from "./verify-win-bundle.mjs";

const conf = (name) =>
  JSON.parse(readFileSync(path.join(import.meta.dirname, "..", "src-tauri", name), "utf8"));
const pkg = JSON.parse(readFileSync(path.join(import.meta.dirname, "..", "package.json"), "utf8"));

describe("bundle config (AD-13)", () => {
  it("keeps the macOS bundle targets exactly app and dmg", () => {
    expect(conf("tauri.conf.json").bundle.targets).toEqual(["app", "dmg"]);
  });

  it("builds only NSIS on Windows and never names an externalBin there", () => {
    const windows = conf("tauri.windows.conf.json");
    expect(windows.bundle.targets).toEqual(["nsis"]);
    // tauri-build merges this file into every Windows-target cargo build, and a missing sidecar
    // file would fail each of them, including the build of dcv2-hook itself.
    expect(JSON.stringify(windows)).not.toContain("externalBin");
    expect(JSON.stringify(conf("tauri.conf.json"))).not.toContain("externalBin");
    expect(conf("tauri.conf.json").bundle.icon).toContain("icons/icon.ico");
  });

  it("has a release:win script that verifies the bundle, and release:mac is untouched", () => {
    expect(pkg.scripts["release:mac"]).toBe("tauri build --bundles app,dmg");
    expect(pkg.scripts["release:win"]).toBe(
      "tauri build --bundles nsis && node scripts/verify-win-bundle.mjs",
    );
  });
});

describe("verify-win-bundle", () => {
  it("passes an installer script that installs dcv2-hook.exe", () => {
    const script = [
      'File "C:\\build\\target\\release\\dcterminalv2.exe"',
      'File "C:\\build\\target\\release\\dcv2-hook.exe"',
    ].join("\r\n");
    expect(checkInstallerScript(script).ok).toBe(true);
  });

  it("fails with the exact message when it does not", () => {
    const script = 'File "C:\\build\\target\\release\\dcterminalv2.exe"\n; dcv2-hook.exe is only a comment\n';
    const result = checkInstallerScript(script);
    expect(result.ok).toBe(false);
    expect(result.message).toBe("dcv2-hook.exe is not in the NSIS bundle");
    expect(MISSING_MESSAGE).toBe("dcv2-hook.exe is not in the NSIS bundle");
  });
});
