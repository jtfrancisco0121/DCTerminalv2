import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";
import { ensureWebStorage } from "./storage/webStorage";

// Node 22 has no Web Storage global. Node 25+ has one that is undefined
// unless --localstorage-file is set, and that shadows jsdom. Install a
// usable store before any test reads localStorage.
ensureWebStorage();

afterEach(() => {
  cleanup();
});
