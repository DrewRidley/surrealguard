import { defineConfig } from "vitest/config";
import { fileURLToPath } from "node:url";

// Resolve the workspace packages to their source so tests run without a build
// step, and use the automatic JSX runtime + jsdom for React component tests.
export default defineConfig({
  esbuild: { jsx: "automatic" },
  test: { environment: "jsdom" },
  resolve: {
    alias: {
      "@surrealdb/analyzer-client": fileURLToPath(
        new URL("../client/src/index.ts", import.meta.url),
      ),
      "@surrealdb/analyzer-query": fileURLToPath(
        new URL("../query/src/index.ts", import.meta.url),
      ),
    },
  },
});
