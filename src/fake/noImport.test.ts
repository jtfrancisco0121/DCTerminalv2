import { describe, expect, it } from "vitest";

// The faked backend (npm run dev:fake) is development only: nothing in the app imports it, and main.tsx loads it only
// behind MODE === "fake" by a dynamic import.
const sources = import.meta.glob("/src/**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;
const IMPORTS_FAKE =
  /from\s+["'][^"']*\/fake\/|import\(\s*["'][^"']*\/fake\/|from\s+["']\.\/fake\b/;

describe("the faked backend stays out of the app", () => {
  it("no file under src/ outside src/fake and main.tsx imports fake/", () => {
    const files = Object.entries(sources);
    expect(files.length).toBeGreaterThan(50);
    const offenders = files
      .filter(
        ([path]) => !path.startsWith("/src/fake/") && path !== "/src/main.tsx",
      )
      .filter(([, text]) => IMPORTS_FAKE.test(text))
      .map(([path]) => path);
    expect(offenders).toEqual([]);
  });

  it("main.tsx loads it only behind MODE === fake, by a dynamic import", () => {
    const main = sources["/src/main.tsx"];
    expect(main).toContain('import.meta.env.MODE === "fake"');
    expect(main).toMatch(/import\(\s*"\.\/fake\/install"\s*\)/);
    expect(main).not.toMatch(/^import .*fake/m);
  });
});
