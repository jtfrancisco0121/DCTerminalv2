import { afterEach, describe, expect, it } from "vitest";
import { ensureWebStorage, isUsableStorage } from "./webStorage";

type StorageKey = "localStorage" | "sessionStorage";

const saved = new Map<StorageKey, PropertyDescriptor | undefined>();

function remember(key: StorageKey) {
  if (saved.has(key)) return;
  saved.set(key, Object.getOwnPropertyDescriptor(globalThis, key));
}

function restore() {
  for (const [key, desc] of saved) {
    if (desc) Object.defineProperty(globalThis, key, desc);
    else {
      delete (globalThis as Record<string, unknown>)[key];
    }
  }
  saved.clear();
}

function brokenGetter() {
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    enumerable: true,
    get() {
      return undefined;
    },
  });
}

afterEach(() => {
  restore();
});

describe("web storage fallback", () => {
  it("installs an in-memory localStorage when the global is missing", () => {
    remember("localStorage");
    delete (globalThis as { localStorage?: unknown }).localStorage;
    const storage = ensureWebStorage().localStorage;
    storage.setItem("pad", "hello");
    expect(storage.getItem("pad")).toBe("hello");
    expect(storage.getItem("missing")).toBeNull();
    storage.clear();
    expect(storage.getItem("pad")).toBeNull();
    expect(typeof localStorage.clear).toBe("function");
    localStorage.clear();
  });

  it("replaces a Node-style getter that returns undefined", () => {
    remember("localStorage");
    brokenGetter();
    expect(isUsableStorage(globalThis.localStorage)).toBe(false);
    const storage = ensureWebStorage().localStorage;
    expect(storage.clear).toEqual(expect.any(Function));
    storage.setItem("k", "v");
    expect(localStorage.getItem("k")).toBe("v");
    localStorage.clear();
    expect(localStorage.getItem("k")).toBeNull();
  });

  it("replaces an empty proxy that answers undefined to every read", () => {
    remember("localStorage");
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      enumerable: true,
      writable: true,
      value: new Proxy(
        {},
        {
          get: () => undefined,
        },
      ),
    });
    expect(() => localStorage.clear()).toThrow();
    const storage = ensureWebStorage().localStorage;
    expect(() => storage.clear()).not.toThrow();
    storage.setItem("tab", "a");
    expect(storage.getItem("tab")).toBe("a");
  });

  it("replaces a getter that throws", () => {
    remember("localStorage");
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      enumerable: true,
      get() {
        throw new Error("localStorage is not available");
      },
    });
    const storage = ensureWebStorage().localStorage;
    storage.setItem("draft", "ok");
    expect(storage.getItem("draft")).toBe("ok");
  });

  it("leaves a working store in place", () => {
    remember("localStorage");
    const working = {
      get length() {
        return 0;
      },
      clear() {},
      getItem(key: string) {
        return key === "__dct_storage_probe__" ? "1" : "kept";
      },
      key() {
        return null;
      },
      removeItem() {},
      setItem() {},
    };
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      enumerable: true,
      writable: true,
      value: working,
    });
    expect(ensureWebStorage().localStorage).toBe(working);
  });

  it("installs sessionStorage the same way", () => {
    remember("sessionStorage");
    delete (globalThis as { sessionStorage?: unknown }).sessionStorage;
    const storage = ensureWebStorage().sessionStorage;
    storage.setItem("s", "1");
    expect(storage.getItem("s")).toBe("1");
    expect(typeof sessionStorage.clear).toBe("function");
  });
});
