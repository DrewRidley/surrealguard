import { defineWorkspace } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { svelteTesting } from "@testing-library/svelte/vite";
import { fileURLToPath } from "node:url";

// Two projects, because SSR is not a per-file flag. Whether `svelte` resolves
// to its client or its server build is decided by Vite's export CONDITIONS,
// which are per-project: a `// @vitest-environment node` docblock changes the
// globals under a test but still hands it the browser build of Svelte, and the
// component then fails at `getContext`.
//
// So: `dom` mounts components (the browser condition, jsdom, testing-library),
// and `ssr` renders them to a string with `svelte/server` (no browser
// condition, node). Same plugin, same aliases, same framework — only the
// resolution differs, which is exactly what is under test.

const alias = {
  "@surrealguard/client": fileURLToPath(new URL("../client/src/index.ts", import.meta.url)),
  "@surrealguard/query": fileURLToPath(new URL("../query/src/index.ts", import.meta.url)),
};

export default defineWorkspace([
  {
    plugins: [svelte(), svelteTesting()],
    resolve: { alias },
    test: {
      name: "dom",
      environment: "jsdom",
      include: ["test/**/*.test.ts"],
      exclude: ["test/ssr.test.ts"],
    },
  },
  {
    plugins: [svelte()],
    resolve: { alias, conditions: ["node", "import", "module", "default"] },
    test: {
      name: "ssr",
      environment: "node",
      include: ["test/ssr.test.ts"],
    },
  },
]);
