// The demo is a browser app: the SurrealDB connection is a WebSocket opened
// from the page, so there is nothing for a server render to do except open a
// second one. `@surrealguard/svelte` supports SSR (`preload` / `hydrate`, and
// `packages/svelte`'s tests cover it) — this example just does not use it, so
// there is one fewer thing to go wrong in front of an audience.
export const ssr = false;
export const prerender = false;
