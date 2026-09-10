import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";
import { surrealguard } from "@surrealguard/svelte/preprocess";

/**
 * The demo runs entirely in the browser — see `src/routes/+layout.ts`, which
 * turns SSR off. `adapter-static` with an SPA fallback is the honest build for
 * that: `pnpm build` emits a static bundle and `pnpm preview` serves it, with
 * no Node server that would need its own SurrealDB connection.
 *
 * @type {import("@sveltejs/kit").Config}
 */
export default {
  preprocess: [surrealguard(), vitePreprocess()],
  kit: {
    adapter: adapter({ fallback: "index.html" }),
  },
};
