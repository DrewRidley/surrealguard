/**
 * Svelte 5 / SvelteKit bindings for SurrealQL Analyzer.
 *
 * - {@link setClient} / {@link useClient} — provide the typed client via context.
 * - {@link createQuery} — a one-shot query with loading and error state.
 * - {@link createLive} — a live query; `data` stays reconciled.
 * - `<Query>` / `<LiveQuery>` — the same two, in markup, with the row type
 *   flowing into the `children` snippet.
 * - {@link createMutation} — a write plus the invalidation that follows it.
 * - {@link preload} — SSR data that remembers which query it is.
 *
 * The runes primitives live in `.svelte.ts` modules so `$derived` / `$state`
 * compile. They use `createSubscriber` rather than `$effect`, so a shared query
 * can live in your own `.svelte.ts` module without `effect_orphan`.
 */

export { default as Query } from "./Query.svelte";
export { default as LiveQuery } from "./LiveQuery.svelte";
export { setClient, useClient } from "./context.js";
export { resolveSource, type Source } from "./source.js";
export {
  createLive,
  createQuery,
  keyOf,
  type CreateOptions,
  type LiveHandle,
  type QueryHandle,
  type QueryStatus,
} from "./queries.svelte.js";
export {
  createMutation,
  type MutationHandle,
  type MutationOptions,
} from "./mutation.svelte.js";
export { dehydrate, hydrate, preload } from "./ssr.js";
export type { Json, Preloaded, SurqlLive, SurqlQuery } from "@surrealdb/analyzer-client";
export { SurrealQLAnalyzerError } from "@surrealdb/analyzer-client";
