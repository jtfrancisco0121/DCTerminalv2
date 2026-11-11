// @vitest-environment jsdom
import { readSource } from "./readSource";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  applyTheme,
  cachedTheme,
  DEFAULT_THEME,
  isLightTheme,
  cachedThemePair,
  DEFAULT_PAIR,
  LEGACY_PAIR,
  normalizePair,
  pairOf,
  resolveTheme,
  normalizeTheme,
  TERMINAL_THEMES,
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

  it("defaults to Soft Dusk and offers the pair, GitHub's pair and four more presets", () => {
    expect(DEFAULT_THEME).toBe("soft-dusk");
    expect(THEMES.map((t) => t.id)).toEqual([
      "soft-dusk",
      "soft-dusk-light",
      "github-dark",
      "github-light",
      "dracula",
      "nord",
      "tokyo-night",
      "catppuccin-mocha",
    ]);
    expect(normalizeTheme("nord")).toBe("nord");
    expect(THEMES[0].label).toBe("Soft Dusk");
    expect(THEMES[1].label).toBe("Soft Dusk Light");
    expect(normalizeTheme("github-light")).toBe("github-light");
    expect(normalizeTheme("github-dark")).toBe("github-dark");
    expect(normalizeTheme("solarized")).toBe("soft-dusk");
    expect(normalizeTheme(undefined)).toBe("soft-dusk");
  });

  it("switches the root data-theme and remembers it for the next start", () => {
    applyTheme("github-light", LEGACY_PAIR);
    expect(document.documentElement.dataset.theme).toBe("github-light");
    expect(document.documentElement.style.colorScheme).toBe("light");
    expect(cachedTheme()).toBe("github-light");
    applyTheme("github-dark", LEGACY_PAIR);
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
      expect(applyTheme("system", LEGACY_PAIR)).toBe("github-light");
      expect(document.documentElement.dataset.theme).toBe("github-light");
      expect(cachedTheme()).toBe("system");
      const stop = watchSystemTheme(() => applyTheme("system", LEGACY_PAIR));
      dark = true;
      listeners.forEach((fn) => fn());
      expect(document.documentElement.dataset.theme).toBe("github-dark");
      expect(terminalTheme("system", LEGACY_PAIR).background).toBe("#0d1117");
      stop();
      expect(listeners.size).toBe(0);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("gives the terminal a matching palette", () => {
    expect(terminalTheme("github-dark", LEGACY_PAIR).background).toBe("#0d1117");
    expect(terminalTheme("github-light", LEGACY_PAIR).background).toBe("#ffffff");
    expect(terminalTheme("github-light", LEGACY_PAIR).foreground).toBe("#1f2328");
    expect(terminalTheme("dracula", LEGACY_PAIR).background).toBe("#282a36");
  });
});

describe("Match system pair", () => {
  const stubOs = (dark: boolean) =>
    vi.stubGlobal("matchMedia", () => ({ matches: dark, addEventListener: () => {}, removeEventListener: () => {} }));
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
    document.documentElement.removeAttribute("data-theme");
  });

  it("resolves Match system through the chosen pair", () => {
    const pair = { dark: "nord", light: "soft-dusk-light" } as const;
    stubOs(true);
    expect(resolveTheme("system", pair)).toBe("nord");
    expect(terminalTheme("system", pair).background).toBe("#2e3440");
    stubOs(false);
    expect(resolveTheme("system", pair)).toBe("soft-dusk-light");
    expect(isLightTheme("system", pair)).toBe(true);
    expect(resolveTheme("dracula", pair)).toBe("dracula");
  });

  it("refuses a theme of the wrong scheme in a pair", () => {
    expect(normalizePair({ dark: "github-light", light: "nord" })).toEqual(DEFAULT_PAIR);
    expect(normalizePair({ dark: "bogus", light: "github-light" })).toEqual({ dark: "soft-dusk", light: "github-light" });
    expect(normalizePair(undefined, LEGACY_PAIR)).toEqual(LEGACY_PAIR);
    expect(pairOf({ systemDark: "dracula", systemLight: "github-light" })).toEqual({ dark: "dracula", light: "github-light" });
    expect(pairOf({})).toEqual(DEFAULT_PAIR);
  });

  it("paints the cached pair first, and the GitHub pair when no pair was cached", () => {
    stubOs(false);
    applyTheme("system", { dark: "nord", light: "soft-dusk-light" });
    expect(cachedThemePair()).toEqual({ dark: "nord", light: "soft-dusk-light" });
    expect(applyTheme(cachedTheme(), cachedThemePair())).toBe("soft-dusk-light");
    // An existing user's cache: the choice is "system" and no pair was ever stored.
    localStorage.clear();
    localStorage.setItem("dcterminalv2.theme", "system");
    expect(cachedThemePair()).toEqual(LEGACY_PAIR);
    expect(applyTheme(cachedTheme(), cachedThemePair())).toBe("github-light");
    stubOs(true);
    expect(applyTheme(cachedTheme(), cachedThemePair())).toBe("github-dark");
    localStorage.setItem("dcterminalv2.themePair", "{not json");
    expect(cachedThemePair()).toEqual(LEGACY_PAIR);
  });
});

describe("theme parity", () => {
  const css = readSource("App.css");

  it("gives every theme a terminal palette and an App.css block", () => {
    expect(THEMES).toHaveLength(8);
    for (const { id } of THEMES) {
      expect(TERMINAL_THEMES[id], `${id} terminal palette`).toBeTruthy();
      expect(css, `${id} css block`).toMatch(new RegExp(`:root\\[data-theme="${id}"\\]\\s*\\{`));
    }
    expect(Object.keys(TERMINAL_THEMES).sort()).toEqual(THEMES.map((t) => t.id).sort());
  });

  it("defines the same colour tokens in every theme block as Soft Dusk", () => {
    const block = (id: string) => {
      const start = css.indexOf(`:root[data-theme="${id}"] {`);
      return css.slice(start, css.indexOf("\n}", start));
    };
    const colourTokens = (text: string) =>
      [...text.matchAll(/^ {2}(--[a-z-]+): ([^;]+);/gm)]
        .filter((m) => /^(#[0-9a-f]+|rgba?\(.*\)|var\(--[a-z-]+\))$/i.test(m[2].trim()))
        .map((m) => m[1]);
    const dusk = new Set(colourTokens(block("soft-dusk")));
    for (const { id } of THEMES) {
      const have = new Set(colourTokens(block(id)));
      for (const token of dusk) expect(have.has(token), `${id} defines ${token}`).toBe(true);
    }
  });
});

describe("Soft Dusk contrast", () => {
  const css = readSource("App.css");
  const block = (head: string) => {
    const start = css.indexOf(head);
    return css.slice(start, css.indexOf("\n}", start));
  };
  const token = (text: string, name: string) => new RegExp(`${name}: (#[0-9a-fA-F]{6});`).exec(text)?.[1] as string;
  const channel = (c: number) => (c / 255 <= 0.03928 ? c / 255 / 12.92 : ((c / 255 + 0.055) / 1.055) ** 2.4);
  const luminance = (hex: string) => {
    const n = parseInt(hex.slice(1), 16);
    return 0.2126 * channel((n >> 16) & 255) + 0.7152 * channel((n >> 8) & 255) + 0.0722 * channel(n & 255);
  };
  const contrast = (a: string, b: string) => {
    const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
    return (hi + 0.05) / (lo + 0.05);
  };
  const surfaces = ["--bg", "--bg-elev", "--bg-inset", "--bg-muted", "--bg-hover"];

  it.each([
    ['soft-dusk', ':root,\n:root[data-theme="soft-dusk"] {'],
    ["soft-dusk-light", ':root[data-theme="soft-dusk-light"] {'],
  ])("%s: body text 11 to 12.5:1, muted text 4.5:1 on each surface, no #000000 background", (_id, head) => {
    const text = block(head);
    const ratio = contrast(token(text, "--fg"), token(text, "--bg"));
    expect(ratio).toBeGreaterThanOrEqual(11);
    expect(ratio).toBeLessThanOrEqual(12.5);
    for (const surface of surfaces) {
      expect(token(text, surface).toLowerCase(), surface).not.toBe("#000000");
      expect(contrast(token(text, "--fg-muted"), token(text, surface)), surface).toBeGreaterThanOrEqual(4.5);
    }
  });
});

describe("light themes", () => {
  it("tells light from dark, so Claude Code can be told the scheme", () => {
    expect(isLightTheme("github-light", LEGACY_PAIR)).toBe(true);
    expect(isLightTheme("soft-dusk-light", LEGACY_PAIR)).toBe(true);
    expect(isLightTheme("soft-dusk", LEGACY_PAIR)).toBe(false);
    expect(isLightTheme("nord", LEGACY_PAIR)).toBe(false);
    expect(isLightTheme("nope", LEGACY_PAIR)).toBe(false);
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
    const light = terminalTheme("github-light", LEGACY_PAIR);
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
