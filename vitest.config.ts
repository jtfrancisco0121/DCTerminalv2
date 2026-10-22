import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "node",
    // scripts/*.test.mjs is the installer and Windows `npm run check` quoting
    // suite. Master has no vitest.config.ts, so Vitest's default include ran
    // those files. e2e/specs stays out: app.spec.mjs reads a temp folder at import.
    include: ["src/**/*.{test,spec}.{ts,tsx}", "scripts/**/*.{test,spec}.mjs"],
    setupFiles: ["src/test-setup.ts"],
  },
});
