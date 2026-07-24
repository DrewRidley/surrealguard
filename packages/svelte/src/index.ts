/**
 * Svelte 5 / SvelteKit bindings for SurrealGuard.
 *
 * - {@link setClient} / {@link getClient} — provide the typed client via context.
 * - {@link liveQuery} — runes-reactive live queries; read `.data` directly.
 * - {@link loadLive} / {@link dehydrate} / {@link hydrate} — SSR seed + hydrate.
 *
 * See `./live-query.svelte.ts` for the runes wrapper (kept in a `.svelte.ts`
 * module so `$state` / `$effect` compile).
 */

export { setClient, getClient } from "./context.js";
export {
  liveQuery,
  type LiveQuery,
  type LiveQueryOptions,
} from "./live-query.svelte.js";
export { loadLive, dehydrate, hydrate } from "./load.js";
