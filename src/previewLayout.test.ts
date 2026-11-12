// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  DEFAULT_PREVIEW_RATIO,
  loadPreviewRatio,
  MAX_PREVIEW_RATIO,
  MIN_PREVIEW_RATIO,
  previewFit,
  previewKey,
  savePreviewRatio,
  splitNarrowestShare,
  terminalAreaMin,
  type PreviewFitInput,
} from "./previewLayout";

afterEach(() => {
  window.localStorage.clear();
  vi.restoreAllMocks();
});

describe("the preview ratio store", () => {
  it("is keyed per window and defaults to 0.42", () => {
    expect(previewKey("main")).toBe("dcv2.preview.main");
    expect(loadPreviewRatio("main")).toBe(DEFAULT_PREVIEW_RATIO);
    expect(DEFAULT_PREVIEW_RATIO).toBe(0.42);
  });
  it("saves {ratio} and reads it back", () => {
    savePreviewRatio("main", 0.5);
    expect(window.localStorage.getItem("dcv2.preview.main")).toBe('{"ratio":0.5}');
    expect(loadPreviewRatio("main")).toBe(0.5);
    expect(loadPreviewRatio("other")).toBe(DEFAULT_PREVIEW_RATIO);
  });
  it("clamps to 0.15..0.85", () => {
    savePreviewRatio("main", 0.99);
    expect(loadPreviewRatio("main")).toBe(MAX_PREVIEW_RATIO);
    window.localStorage.setItem("dcv2.preview.main", '{"ratio":0.01}');
    expect(loadPreviewRatio("main")).toBe(MIN_PREVIEW_RATIO);
  });
  it("ignores junk", () => {
    for (const raw of ["nope", "null", "7", '{"ratio":"x"}', "{}", '{"ratio":null}']) {
      window.localStorage.setItem("dcv2.preview.main", raw);
      expect(loadPreviewRatio("main")).toBe(DEFAULT_PREVIEW_RATIO);
    }
  });
  it("survives storage that throws", () => {
    const real = Object.getOwnPropertyDescriptor(window, "localStorage");
    const deny = () => {
      throw new Error("denied");
    };
    Object.defineProperty(window, "localStorage", { value: { getItem: deny, setItem: deny }, configurable: true });
    try {
      expect(loadPreviewRatio("main")).toBe(DEFAULT_PREVIEW_RATIO);
      expect(() => savePreviewRatio("main", 0.3)).not.toThrow();
    } finally {
      if (real) Object.defineProperty(window, "localStorage", real);
      else delete (window as { localStorage?: unknown }).localStorage;
    }
  });
});

describe("previewFit", () => {
  // Cells are 10px, padding 6px: 60 columns need 612px. Minimum preview 320px.
  const base: PreviewFitInput = {
    ratio: 0.42,
    windowWidth: 1400,
    rowWidth: 1400,
    runPanelWidth: 0,
    gutter: 8,
    cellWidth: 10,
    termPad: 6,
    minPreviewPx: 320,
  };
  it("is an overlay under 1000px and beside the terminals at 1000px", () => {
    expect(previewFit({ ...base, windowWidth: 999, rowWidth: 999 }).mode).toBe("overlay");
    expect(previewFit({ ...base, windowWidth: 1000, rowWidth: 1000 }).mode).toBe("side");
  });
  it("takes the ratio of the available width", () => {
    // available 1392; 0.42 of it is 585 (rounded)
    expect(previewFit(base)).toEqual({ mode: "side", width: 585 });
  });
  it("is an overlay when 60 columns plus the preview minimum do not fit", () => {
    // 612 + 320 = 932 needed, plus one 8px gutter
    expect(previewFit({ ...base, windowWidth: 1100, rowWidth: 940 }).mode).toBe("side");
    expect(previewFit({ ...base, windowWidth: 1100, rowWidth: 939 }).mode).toBe("overlay");
  });
  it("clamps the width so the terminals keep exactly 60 columns, and no fewer", () => {
    const fit = previewFit({ ...base, ratio: 0.85, rowWidth: 1100, windowWidth: 1100 });
    expect(fit).toEqual({ mode: "side", width: 1092 - 612 });
    for (const ratio of [0.15, 0.3, 0.5, 0.7, 0.85]) {
      for (const rowWidth of [940, 1000, 1200, 1600]) {
        const result = previewFit({ ...base, ratio, rowWidth, windowWidth: rowWidth });
        if (result.mode !== "side") continue;
        const terminals = rowWidth - 8 - result.width;
        expect(terminals).toBeGreaterThanOrEqual(612);
        expect(result.width).toBeGreaterThanOrEqual(320);
      }
    }
  });
  it("never goes under the preview minimum for a small ratio", () => {
    expect(previewFit({ ...base, ratio: 0.15, rowWidth: 1000, windowWidth: 1000 })).toEqual({ mode: "side", width: 320 });
  });
  it("leaves room for the run panel when it is open and not when it is collapsed", () => {
    const open = previewFit({ ...base, runPanelWidth: 320 }) as { mode: "side"; width: number };
    const closed = previewFit({ ...base, runPanelWidth: 0 }) as { mode: "side"; width: number };
    // open: available 1400 - 320 - 16 = 1064; closed: 1392
    expect(open.width).toBe(Math.round(0.42 * 1064));
    expect(closed.width).toBe(Math.round(0.42 * 1392));
    expect(open.width).toBeLessThan(closed.width);
    // With the panel open the narrow row no longer fits 60 columns plus the preview.
    expect(previewFit({ ...base, runPanelWidth: 320, rowWidth: 1200, windowWidth: 1200 }).mode).toBe("overlay");
  });
  it("is pure: the fit never writes storage and the ratio is not changed by the clamp", () => {
    savePreviewRatio("main", 0.85);
    const stored = window.localStorage.getItem("dcv2.preview.main");
    const clamped = previewFit({ ...base, ratio: 0.85, rowWidth: 1000, windowWidth: 1000 });
    expect(clamped).toEqual({ mode: "side", width: 1000 - 8 - 612 });
    expect(window.localStorage.getItem("dcv2.preview.main")).toBe(stored);
    expect(loadPreviewRatio("main")).toBe(0.85);
  });

  describe("in a tab split side by side", () => {
    // The split's divider and pane borders: 9px.
    const CHROME = 9;
    const panesOf = (rowWidth: number, width: number, splitRatio: number) => {
      const inner = rowWidth - 8 - width - CHROME;
      return [splitRatio * inner, (1 - splitRatio) * inner];
    };

    it("keeps 60 columns in each pane, not in the two together", () => {
      const split = { narrowestShare: splitNarrowestShare(0.5), chromePx: CHROME };
      // One pane: 612px of terminals is enough. Two halves: each needs 612, so the area needs 2 * 612 + 9 = 1233.
      expect(terminalAreaMin(base)).toBe(612);
      expect(terminalAreaMin({ ...base, split })).toBe(1233);
      const fit = previewFit({ ...base, rowWidth: 2000, windowWidth: 2000, split });
      expect(fit).toEqual({ mode: "side", width: 1992 - 1233 });
      const [left, right] = panesOf(2000, (fit as { width: number }).width, 0.5);
      expect(left).toBeGreaterThanOrEqual(612);
      expect(right).toBeGreaterThanOrEqual(612);
    });

    it("falls back to the overlay when the panes would go under 60 columns, where one pane would still fit beside", () => {
      const split = { narrowestShare: splitNarrowestShare(0.5), chromePx: CHROME };
      expect(previewFit(base)).toEqual({ mode: "side", width: 585 });
      expect(previewFit({ ...base, split }).mode).toBe("overlay");
      // 1233 + 320 + 8 = 1561 is the narrowest row that fits.
      expect(previewFit({ ...base, rowWidth: 1561, windowWidth: 1561, split }).mode).toBe("side");
      expect(previewFit({ ...base, rowWidth: 1560, windowWidth: 1560, split }).mode).toBe("overlay");
    });

    it("counts the narrower pane of an uneven split, for any ratio and row width", () => {
      expect(splitNarrowestShare(0.7)).toBeCloseTo(0.3);
      expect(splitNarrowestShare(0.3)).toBeCloseTo(0.3);
      expect(splitNarrowestShare(0.05)).toBe(0.2);
      expect(splitNarrowestShare(Number.NaN)).toBe(0.5);
      for (const splitRatio of [0.2, 0.35, 0.5, 0.65, 0.8]) {
        const split = { narrowestShare: splitNarrowestShare(splitRatio), chromePx: CHROME };
        for (const ratio of [0.15, 0.42, 0.85]) {
          for (const rowWidth of [1400, 2000, 2600, 3400, 4200]) {
            const result = previewFit({ ...base, ratio, rowWidth, windowWidth: rowWidth, split });
            if (result.mode !== "side") continue;
            // Within float error of the share's product: a pane of exactly 612px may compute as 611.9999999.
            for (const pane of panesOf(rowWidth, result.width, splitRatio)) expect(pane).toBeGreaterThanOrEqual(612 - 1e-9);
            expect(result.width).toBeGreaterThanOrEqual(320);
          }
        }
      }
    });

    it("leaves a split stacked top and bottom (no split input) to the one-pane rule", () => {
      expect(previewFit({ ...base, split: null })).toEqual(previewFit(base));
    });
  });
});
