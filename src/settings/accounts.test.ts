import { describe, expect, it } from "vitest";
import { accountIdFor, accountsProblem, addAccount, MAX_ACCOUNTS } from "./accounts";

describe("account ids", () => {
  it("are made from the name, safe for the backend", () => {
    expect(accountIdFor("Work 2", [])).toBe("work-2");
    expect(accountIdFor("  Koneksi (QA)! ", [])).toBe("koneksi-qa");
    expect(accountIdFor("Café", [])).toBe("cafe");
    expect(accountIdFor("!!!", [])).toBe("account");
    expect(accountIdFor("x".repeat(100), [])).toHaveLength(40);
    expect(accountIdFor("Work 2", [])).toMatch(/^[A-Za-z0-9_-]+$/);
  });

  it("never reuse a taken id", () => {
    expect(accountIdFor("Work", ["work"])).toBe("work-2");
    expect(accountIdFor("Work", ["work", "work-2"])).toBe("work-3");
    expect(accountIdFor("Default", ["default"])).toBe("default-2");
  });
});

describe("the accounts list", () => {
  it("adds an account with a fresh id and an optional folder", () => {
    const start = [{ id: "default", name: "Claude" }];
    const next = addAccount(start, " Work ", " ~/.claude-account2 ");
    expect(next).toEqual([
      { id: "default", name: "Claude" },
      { id: "work", name: "Work", configDir: "~/.claude-account2" },
    ]);
    expect(addAccount(next, "", "")[2]).toEqual({ id: "claude", name: "Claude", configDir: null });
    expect(start).toHaveLength(1);
  });

  it("says why a list cannot be saved", () => {
    expect(accountsProblem([])).toBe("Keep at least one account.");
    expect(accountsProblem([{ id: "a", name: " " }])).toBe("Every account needs a name.");
    const many = Array.from({ length: MAX_ACCOUNTS + 1 }, (_, n) => ({ id: `a${n}`, name: "A" }));
    expect(accountsProblem(many)).toContain(`${MAX_ACCOUNTS}`);
    expect(accountsProblem([{ id: "a", name: "A" }])).toBeNull();
  });
});
