// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";

const ptySetScheme = vi.fn((_light: boolean) => Promise.resolve());
vi.mock("../bridge", () => ({ ptySetScheme: (light: boolean) => ptySetScheme(light) }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class {} }));
vi.mock("@xterm/addon-search", () => ({ SearchAddon: class {} }));
vi.mock("@xterm/addon-web-links", () => ({ WebLinksAddon: class {} }));
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    options: Record<string, unknown> = {};
    loadAddon() {}
    registerLinkProvider() {}
    onSelectionChange() {}
    onData() {
      return { dispose() {} };
    }
    open() {}
  },
}));

import { applyTheme } from "../theme";
import { ensureParkedTerminal, setTerminalTheme } from "./park";

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
});

describe("terminal theme and Match system", () => {
  it("recolours a parked terminal with the chosen pair when the OS switches", () => {
    let dark = true;
    vi.stubGlobal("matchMedia", () => ({ get matches() { return dark; }, addEventListener() {}, removeEventListener() {} }));
    const pair = { dark: "nord", light: "soft-dusk-light" } as const;
    const { term } = ensureParkedTerminal("t-theme", 13);
    applyTheme("system", pair);
    setTerminalTheme("system", pair);
    expect((term.options.theme as { background: string }).background).toBe("#2e3440");
    expect(ptySetScheme).toHaveBeenLastCalledWith(false);

    dark = false;
    applyTheme("system", pair);
    setTerminalTheme("system", pair);
    expect((term.options.theme as { background: string }).background).toBe("#fbfbfc");
    expect(ptySetScheme).toHaveBeenLastCalledWith(true);
  });
});
