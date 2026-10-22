/**
 * Node 25+ exposes `localStorage` as a global. Without `--localstorage-file`
 * that getter warns and returns undefined, which hides jsdom's Storage.
 * Draft saves and Vitest both need a store that implements the DOM methods.
 */

const PROBE_KEY = "__dct_storage_probe__";

type StorageKey = "localStorage" | "sessionStorage";

const memoryByKey = new Map<StorageKey, Storage>();

function memoryStorage(): Storage {
  const store = new Map<string, string>();
  return {
    get length() {
      return store.size;
    },
    clear() {
      store.clear();
    },
    getItem(key: string) {
      const value = store.get(String(key));
      return value === undefined ? null : value;
    },
    key(index: number) {
      return [...store.keys()][index] ?? null;
    },
    removeItem(key: string) {
      store.delete(String(key));
    },
    setItem(key: string, value: string) {
      store.set(String(key), String(value));
    },
  };
}

/** True when get/set/remove/clear work. An empty proxy or undefined does not. */
export function isUsableStorage(storage: unknown): storage is Storage {
  if (!storage || typeof storage !== "object") return false;
  const candidate = storage as Storage;
  if (typeof candidate.getItem !== "function") return false;
  if (typeof candidate.setItem !== "function") return false;
  if (typeof candidate.removeItem !== "function") return false;
  if (typeof candidate.clear !== "function") return false;
  try {
    candidate.setItem(PROBE_KEY, "1");
    const ok = candidate.getItem(PROBE_KEY) === "1";
    candidate.removeItem(PROBE_KEY);
    return ok;
  } catch {
    return false;
  }
}

type EmitWarning = (warning: unknown, ...args: unknown[]) => void;

/** Node prints this when its Web Storage getter is read with no file configured. */
function readKey(scope: object, key: StorageKey): unknown {
  const proc = (globalThis as { process?: { emitWarning?: EmitWarning } }).process;
  const original = proc?.emitWarning;
  if (proc && original) {
    proc.emitWarning = (warning: unknown, ...args: unknown[]) => {
      const text =
        typeof warning === "string" ? warning : (warning as { message?: string })?.message;
      if (String(text).includes("--localstorage-file")) return;
      original.call(proc, warning, ...args);
    };
  }
  try {
    return (scope as Record<string, unknown>)[key];
  } catch {
    return undefined;
  } finally {
    if (proc && original) proc.emitWarning = original;
  }
}

function assignKey(scope: object, key: StorageKey, storage: Storage): void {
  try {
    Object.defineProperty(scope, key, {
      configurable: true,
      enumerable: true,
      writable: true,
      value: storage,
    });
    return;
  } catch {
    // A non-configurable getter cannot be replaced in place.
  }
  try {
    delete (scope as Record<string, unknown>)[key];
  } catch {
    // Ignore. The caller still receives the in-memory store.
  }
  try {
    Object.defineProperty(scope, key, {
      configurable: true,
      enumerable: true,
      writable: true,
      value: storage,
    });
  } catch {
    try {
      (scope as Record<string, unknown>)[key] = storage;
    } catch {
      // The returned store is still safe to use.
    }
  }
}

function ensureOne(scope: object, key: StorageKey): Storage {
  const existing = readKey(scope, key);
  if (isUsableStorage(existing)) return existing;
  let memory = memoryByKey.get(key);
  if (!memory) {
    memory = memoryStorage();
    memoryByKey.set(key, memory);
  }
  assignKey(scope, key, memory);
  const windowScope = (scope as { window?: object }).window;
  if (windowScope && windowScope !== scope) assignKey(windowScope, key, memory);
  const installed = readKey(scope, key);
  return isUsableStorage(installed) ? installed : memory;
}

/**
 * Keep a working Web Storage global, or install an in-memory one.
 * Returns the stores the app should read and write, even if the global
 * itself could not be replaced.
 */
export function ensureWebStorage(scope: object = globalThis): {
  localStorage: Storage;
  sessionStorage: Storage;
} {
  return {
    localStorage: ensureOne(scope, "localStorage"),
    sessionStorage: ensureOne(scope, "sessionStorage"),
  };
}
