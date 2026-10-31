import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SRC = join(dirname(fileURLToPath(import.meta.url)), "..", "src");

describe("the context threshold", () => {
  it("is not restated in ObserveSection.tsx (the backend sends warnPercent)", () => {
    const tsx = readFileSync(join(SRC, "components", "ObserveSection.tsx"), "utf8");
    expect(tsx).not.toContain("over 60");
    expect(tsx).not.toMatch(/\b60%/);
  });
});
