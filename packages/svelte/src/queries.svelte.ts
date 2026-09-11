/**
 * `createQuery` / `createLive` — Svelte 5 reactive primitives over the reactive
 * core.
 *
 * The returned object exposes `data` / `status` / `loading` / `error` as
 * **getters**, so you read them directly in markup with no `$` prefix and
 * reading tracks. That shape was already right in 0.4 and is kept.
 *
 * Two things are new and they are the point of the rewrite:
 *
 * 1. **A `Source<Q>` thunk keeps params reactive.** 0.4 read `{ params }` once,
 *    at construction, so a query parameterised on `$derived` state could never
 *    re-run.
 * 2. **`createSubscriber`, not `$effect`.** `$effect` only runs inside a
 *    component, so sharing a query from a `.svelte.ts` module — the idiomatic
 *    Svelte 5 way to share reactive state — threw `effect_orphan` unless the
 *    caller wrapped it in `$effect.root` and managed disposal by hand.
 *    `createSubscriber` (svelte/reactivity, 5.7+) subscribes lazily on first
 *    read inside a tracking scope and tears down automatically, anywhere.
 *
 * ```svelte
 * <script lang="ts">
 *   import { page } from "$app/state";
 *   import { createLive, createQuery } from "@surrealdb/analyzer-svelte";
 *   import { allPeople, liveTeam } from "$lib/queries";
 *
 *   // re-subscribes whenever page.params.team changes; the old LIVE is KILLed
 *   const team = createLive(() => liveTeam.with({ team: page.params.team }));
 *   const roster = createQuery(allPeople);
 * </script>
 * ```
 */

import { createSubscriber } from "svelte/reactivity";
import {
  computeKey,
  type Bound,
  type Json,
  type Preloaded,
  type Rows,
  type SurqlLive,
  type SurqlQuery,
  type SurrealQLAnalyzerClient,
  type SurrealQLAnalyzerError,
} from "@surrealdb/analyzer-client";
import { getQueryClient, type Observable, type QueryState } from "@surrealdb/analyzer-query";
import { useClient } from "./context.js";
import { resolveSource, type Source } from "./source.js";

export type QueryStatus = "pending" | "success" | "error";

/** A one-shot query's reactive view. `data` may be a scalar, so it is optional. */
export interface QueryHandle<T> {
  readonly data: T | undefined;
  readonly error: SurrealQLAnalyzerError | undefined;
  readonly loading: boolean;
  readonly status: QueryStatus;
  refetch(): Promise<void>;
}

/** A live query's reactive view. `data` is always an array: it is a row stream. */
export interface LiveHandle<Row> {
  readonly data: Row[];
  readonly error: SurrealQLAnalyzerError | undefined;
  readonly loading: boolean;
  readonly status: QueryStatus;
}

export interface CreateOptions<T> {
  /** Override the context client (tests, a second connection). */
  client?: SurrealQLAnalyzerClient;
  /** Seed data for a gap-free first render. `preload` supplies this for you. */
  initial?: T;
}

/**
 * Rebuild a query reference from a {@link Preloaded} payload. This is what lets
 * a component subscribe to exactly the query the server ran **without naming
 * it again** — the structural flaw the redesign exists to fix, since 0.4 made
 * `+page.ts` and `+page.svelte` each spell the text out and silently discarded
 * the seed if they ever drifted by a byte.
 */
function fromPreloaded(payload: Preloaded<unknown>): SurqlLive<unknown, Bound> {
  return {
    text: payload.text,
    liveText: payload.liveText ?? `LIVE ${payload.text}`,
    params: payload.params,
    key: payload.key as SurqlLive<unknown, Bound>["key"],
    isLive: payload.isLive,
    with: () => fromPreloaded(payload),
  } as SurqlLive<unknown, Bound>;
}

/**
 * A raw string reaching here means the inline attribute form was written and
 * `@surrealdb/analyzer-svelte/preprocess` is not in the `svelte.config.js` preprocess
 * chain — so Svelte concatenated the attribute and handed us the finished text.
 * That is precisely the case the preprocessor exists to prevent, so it fails
 * loudly rather than running an unparameterised, un-cached query.
 */
function assertPreprocessed(resolved: unknown): void {
  if (typeof resolved !== "string" || resolved === "skip") return;
  throw new Error(
    "[@surrealdb/analyzer-svelte] <Query>/<LiveQuery> received a plain string. Add the " +
      "preprocessor to svelte.config.js:\n\n" +
      '  import { surrealqlAnalyzer } from "@surrealdb/analyzer-svelte/preprocess";\n' +
      "  export default { preprocess: [surrealqlAnalyzer(), vitePreprocess()] };\n\n" +
      `Got: ${JSON.stringify(resolved.slice(0, 80))}`,
  );
}

function isPreloaded(value: unknown): value is Preloaded<unknown> {
  return (
    typeof value === "object" &&
    value !== null &&
    "data" in value &&
    "key" in value &&
    !("with" in value)
  );
}

/** A subscribed observable plus the `createSubscriber` gate that tracks it. */
interface View {
  observable: Observable<unknown>;
  track: () => void;
}

/**
 * Build the reactive view. Wrapped in `$derived.by` by the callers, so the
 * source thunk re-runs when its dependencies change — and when it resolves to a
 * different query key, a fresh observable and subscriber replace the old one,
 * whose teardown fires as soon as nothing reads it.
 */
function makeView(
  client: SurrealQLAnalyzerClient,
  query: SurqlQuery<unknown, Bound> | SurqlLive<unknown, Bound>,
  initial: unknown,
  live: boolean,
): View {
  const core = getQueryClient(client);
  const observable = live
    ? (core.observeLive(query as SurqlLive<unknown, Bound>, {
        initialData: initial as unknown[] | undefined,
      }) as Observable<unknown>)
    : (core.observe(query as SurqlQuery<unknown, Bound>, {
        initialData: initial,
      }) as Observable<unknown>);
  return {
    observable,
    track: createSubscriber((update) => observable.subscribe(() => update())),
  };
}

const PENDING: QueryState<unknown> = { status: "pending", data: undefined, error: undefined };

/**
 * A one-shot query, with loading and error state — the primitive that simply
 * did not exist in 0.4, where `liveQuery` was the only reactive export and
 * "fetch this once, show a spinner, show an error" had nothing to call.
 */
export function createQuery<R>(
  // `| string` is the inline attribute form, which the preprocessor rewrites
  // before it can ever get here; `assertPreprocessed` below says so if it does.
  source: Source<SurqlQuery<R, Bound> | Preloaded<Json<Rows<R>>> | string>,
  options: CreateOptions<Json<Rows<R>>> = {},
): QueryHandle<Json<Rows<R>>> {
  const client = options.client ?? useClient();

  const view = $derived.by((): View | undefined => {
    const resolved = resolveSource(source);
    if (resolved === "skip") return undefined;
    assertPreprocessed(resolved);
    if (isPreloaded(resolved)) {
      return makeView(client, fromPreloaded(resolved), resolved.data, false);
    }
    return makeView(client, resolved as SurqlQuery<unknown, Bound>, options.initial, false);
  });

  const read = (): QueryState<unknown> => {
    const current = view;
    if (!current) return PENDING;
    current.track();
    return current.observable.get();
  };

  return {
    get data() {
      return read().data as Json<Rows<R>> | undefined;
    },
    get error() {
      return read().error;
    },
    get loading() {
      return read().status === "pending";
    },
    get status() {
      return read().status;
    },
    refetch: async () => {
      await view?.observable.refetch();
    },
  };
}

/**
 * A live query. `data` is always an array and starts `[]`, so markup needs no
 * `?? []`.
 *
 * Passing a {@link Preloaded} payload hydrates from the server's rows and then
 * upgrades to live in place, with the query text appearing nowhere in the
 * component. Pass it through a thunk — `createLive(() => data.people)` — if the
 * route's `data` can change under you on a client-side navigation; that is the
 * same reactivity trap the thunk form exists to close.
 */
export function createLive<Row>(
  source: Source<SurqlLive<Row, Bound> | Preloaded<Json<Row>[]> | string>,
  options: CreateOptions<Json<Row>[]> = {},
): LiveHandle<Json<Row>> {
  const client = options.client ?? useClient();

  const view = $derived.by((): View | undefined => {
    const resolved = resolveSource(source);
    if (resolved === "skip") return undefined;
    assertPreprocessed(resolved);
    if (isPreloaded(resolved)) {
      return makeView(client, fromPreloaded(resolved), resolved.data, true);
    }
    return makeView(client, resolved as SurqlLive<unknown, Bound>, options.initial, true);
  });

  const read = (): QueryState<unknown> => {
    const current = view;
    if (!current) return PENDING;
    current.track();
    return current.observable.get();
  };

  return {
    get data() {
      return (read().data ?? []) as Json<Row>[];
    },
    get error() {
      return read().error;
    },
    get loading() {
      return read().status === "pending";
    },
    get status() {
      return read().status;
    },
  };
}

/**
 * The key a source resolves to right now — useful for tests and for debugging a
 * cache miss.
 */
export function keyOf(text: string, params?: Record<string, unknown>): string {
  return computeKey(text, params);
}
