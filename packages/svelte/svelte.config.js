import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

// Minimal config for svelte-package + svelte-check. `vitePreprocess` lets
// `<script lang="ts">` and `.svelte.ts` runes modules be type-checked.
export default {
  preprocess: vitePreprocess(),
};
