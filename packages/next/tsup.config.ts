import { defineConfig } from "tsup";

// Two entries: the client entry (`.`) carries a `"use client"` banner so the
// provider + hooks are a client module in the App Router; the server entry
// (`./server`) stays server-safe (no directive), so an RSC or a route handler
// can call `preload` / `dehydrate`.
export default defineConfig([
  {
    entry: { index: "src/index.ts" },
    format: ["esm", "cjs"],
    dts: true,
    clean: true,
    external: ["react"],
    banner: { js: '"use client";' },
  },
  {
    entry: { server: "src/server.ts" },
    format: ["esm", "cjs"],
    dts: true,
    external: ["react"],
  },
]);
