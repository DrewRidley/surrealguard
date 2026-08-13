import { sveltekit } from "@sveltejs/kit/vite";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [sveltekit()],
  server: {
    // Not 5173: a demo machine usually already has something on the default
    // port, and "the page is blank" ten seconds before you present is not a
    // debugging session worth having.
    port: 5178,
    strictPort: true,
  },
  preview: {
    port: 5179,
    strictPort: true,
  },
});
