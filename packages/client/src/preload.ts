/**
 * `preload` — server-fetched data that *remembers which query it is*.
 *
 * This is the piece that removes the redesign's biggest structural flaw. In 0.4
 * the SSR seed and the component each spelled out the same query text, and if
 * they ever drifted by one byte the cache key stopped matching: the seed was
 * silently discarded, the page refetched, and nothing — no error, no warning, no
 * type failure — said so.
 *
 * A {@link Preloaded} payload carries the key, the text and the parameters
 * alongside the data, so the component reconstructs the subscription from the
 * payload and the text appears exactly once, in `queries.ts`.
 *
 * ```ts
 * // +page.ts
 * export const load = async () => ({ people: await preload(db, livePeople) });
 * ```
 * ```svelte
 * <script lang="ts">
 *   let { data } = $props();
 *   const people = createLive(data.people);   // no query text here at all
 * </script>
 * ```
 *
 * The payload is a plain object of plain values: it survives SvelteKit's
 * devalue and a React Server Component's props boundary without a transport
 * hook, which is why the whole reactive layer is {@link Json}-shaped.
 */

import type { SurrealQLAnalyzerClient } from "./client.js";
import type { Bound, Json, ParamsArg } from "./registry.js";
import type { Rows, SurqlLive, SurqlQuery } from "./query.js";

/**
 * A serialisable, self-describing query result. `T` rides along as a phantom so
 * the row type survives the server → client boundary with no second reference
 * to the query.
 */
export interface Preloaded<T> {
  readonly key: string;
  readonly text: string;
  readonly liveText: string | undefined;
  readonly params: Record<string, unknown> | undefined;
  readonly isLive: boolean;
  readonly data: T;
}

/**
 * Fetch a query on the server and return a payload the client can both render
 * immediately and (for a live query) upgrade in place.
 */
export function preload<Row, P extends Record<string, unknown>>(
  client: SurrealQLAnalyzerClient,
  query: SurqlLive<Row, P>,
  ...args: ParamsArg<P>
): Promise<Preloaded<Json<Row>[]>>;
export function preload<R, P extends Record<string, unknown>>(
  client: SurrealQLAnalyzerClient,
  query: SurqlQuery<R, P>,
  ...args: ParamsArg<P>
): Promise<Preloaded<Json<Rows<R>>>>;
export async function preload(
  client: SurrealQLAnalyzerClient,
  query: SurqlLive<unknown, Record<string, unknown>> | SurqlQuery<unknown, Record<string, unknown>>,
  params?: Record<string, unknown>,
): Promise<Preloaded<unknown>> {
  const { jsonify } = await import("surrealdb");
  const bound = params ? query.with(params) : query;
  const data = bound.isLive
    ? await client.runLiveOnce(bound as SurqlLive<unknown, Bound>)
    : await client.run(bound as SurqlQuery<unknown, Bound>);
  return {
    key: bound.key,
    text: bound.text,
    liveText: bound.isLive ? (bound as SurqlLive<unknown, Bound>).liveText : undefined,
    params: bound.params,
    isLive: bound.isLive,
    data: jsonify(data),
  };
}
