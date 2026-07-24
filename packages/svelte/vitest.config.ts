import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { svelteTesting } from "@testing-library/svelte/vite";
import { fileURLToPath } from "node:url";

// Compile `.svelte` / `.svelte.ts` (runes) with the Svelte plugin; resolve the
// workspace packages to their source so tests run without a build step. jsdom +
// svelteTesting keep component/lifecycle behaviour close to a browser.
export default defineConfig({
  plugins: [svelte(), svelteTesting()],
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
