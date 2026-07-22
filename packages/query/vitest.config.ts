import { defineConfig } from "vitest/config";
import { fileURLToPath } from "node:url";

// Resolve the workspace client to its source so tests run without a build step.
export default defineConfig({
  resolve: {
    alias: {
      "@surrealguard/client": fileURLToPath(
        new URL("../client/src/index.ts", import.meta.url),
      ),
    },
  },
});
