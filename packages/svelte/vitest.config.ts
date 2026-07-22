import { defineConfig } from "vitest/config";
import { fileURLToPath } from "node:url";

// Resolve the workspace packages to their source so tests run without a build
// step; jsdom keeps store/lifecycle behaviour close to a browser.
export default defineConfig({
  test: { environment: "jsdom" },
  resolve: {
    alias: {
      "@surrealguard/client": fileURLToPath(
        new URL("../client/src/index.ts", import.meta.url),
      ),
      "@surrealguard/query": fileURLToPath(
        new URL("../query/src/index.ts", import.meta.url),
      ),
    },
  },
});
