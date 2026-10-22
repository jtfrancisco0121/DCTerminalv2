// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  applyTheme,
  cachedTheme,
  DEFAULT_THEME,
  isLightTheme,
  normalizeTheme,
  terminalTheme,
  THEME_CHOICES,
  THEMES,
  watchSystemTheme,
} from "./theme";

describe("themes", () => {
  afterEach(() => {
    localStorage.clear();
    document.documentElement.removeAttribute("data-theme");
  });

  it("defaults to GitHub Dark and offers the light theme and four dark presets", () => {
    expect(DEFAULT_THEME).toBe("github-dark");
    expect(THEMES.map((t) => t.id)).toEqual([
      "github-dark",
      "github-light",
      "dracula",
      "nord",
      "tokyo-night",
      "catppuccin-mocha",
    ]);
    expect(normalizeTheme("nord")).toBe("nord");
    expect(THEMES[0].label).toBe("GitHub Dark");
    expect(normalizeTheme("github-light")).toBe("github-light");
    expect(normalizeTheme("solarized")).toBe("github-dark");
    expect(normalizeTheme(undefined)).toBe("github-dark");
  });

  it("switches the root data-theme and remembers it for the next start", () => {
    applyTheme("github-light");
    expect(document.documentElement.dataset.theme).toBe("github-light");
    expect(document.documentElement.style.colorScheme).toBe("light");
    expect(cachedTheme()).toBe("github-light");
    applyTheme("github-dark");
    expect(document.documentElement.dataset.theme).toBe("github-dark");
    expect(document.documentElement.style.colorScheme).toBe("dark");
    expect(cachedTheme()).toBe("github-dark");
  });

  it("Match system follows the OS setting and keeps the choice cached", () => {
    let dark = false;
    const listeners = new Set<() => void>();
    vi.stubGlobal("matchMedia", () => ({
      get matches() {
        return dark;
      },
      addEventListener: (_: string, fn: () => void) => listeners.add(fn),
      removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
    }));
    try {
      expect(normalizeTheme("system")).toBe("system");
      expect(THEME_CHOICES[0]).toEqual({ id: "system", label: "Match system" });
      expect(applyTheme("system")).toBe("github-light");
      expect(document.documentElement.dataset.theme).toBe("github-light");
      expect(cachedTheme()).toBe("system");
      const stop = watchSystemTheme(() => applyTheme("system"));
      dark = true;
      listeners.forEach((fn) => fn());
      expect(document.documentElement.dataset.theme).toBe("github-dark");
      expect(terminalTheme("system").background).toBe("#0d1117");
      stop();
      expect(listeners.size).toBe(0);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("gives the terminal a matching palette", () => {
    expect(terminalTheme("github-dark").background).toBe("#0d1117");
    expect(terminalTheme("github-light").background).toBe("#ffffff");
    expect(terminalTheme("github-light").foreground).toBe("#1f2328");
    expect(terminalTheme("dracula").background).toBe("#282a36");
  });
});

describe("light themes", () => {
  it("tells light from dark, so Claude Code can be told the scheme", () => {
    expect(isLightTheme("github-light")).toBe(true);
    expect(isLightTheme("nord")).toBe(false);
    expect(isLightTheme("nope")).toBe(false);
  });
});

describe("light terminal palette (R22)", () => {
  const channel = (value: number) => {
    const c = value / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  const luminance = (hex: string) => {
    const n = parseInt(hex.slice(1), 16);
    return (
      0.2126 * channel((n >> 16) & 255) +
      0.7152 * channel((n >> 8) & 255) +
      0.0722 * channel(n & 255)
    );
  };
  const contrast = (a: string, b: string) => {
    const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
    return (hi + 0.05) / (lo + 0.05);
  };

  it("keeps the dim and bright colors readable (>= 4.5:1) on the light background", () => {
    const light = terminalTheme("github-light");
    for (const key of [
      "yellow",
      "white",
      "brightWhite",
      "brightMagenta",
      "brightBlue",
      "brightCyan",
      "brightYellow",
    ] as const) {
      expect(
        contrast(light[key] as string, light.background as string),
        key,
      ).toBeGreaterThanOrEqual(4.5);
    }
  });
});
