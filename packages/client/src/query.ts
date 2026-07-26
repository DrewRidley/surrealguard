/**
 * A query is a value, not a string.
 *
 * ```ts
 * // src/lib/queries.ts — the one place query text lives
 * import { defineQuery, defineLive } from "./surrealguard.generated";
 *
 * export const allPeople = defineQuery("SELECT id, name, age FROM person");
 * export const peopleOf  = defineQuery("SELECT id, name FROM person WHERE team = $team");
 * export const livePeople = defineLive("SELECT id, name, age FROM person");
 * ```
 *
 * `defineQuery<Q extends string>(text: Q)` is the same inference site as
 * `db.query<Q extends string>(query: Q, …)`: TypeScript infers `Q` as the string
 * literal, one conditional generic reads {@link SurqlRegistry}, and there is no
 * permissive `string` overload. The literal is simply written *once*, in a
 * module, instead of once per consumer — which is what stops an SSR seed and a
 * component from drifting apart byte-for-byte with no error and no warning.
 *
 * The returned {@link SurqlQuery} carries `text`, `params` and a stable `key` as
 * real runtime fields; the result and parameter types ride along as phantoms, in
 * the same spirit as gql.tada's `TadaDocumentNode`.
 */

import { toSurrealqlString } from "surrealdb";
import type {
  Bound,
  ParamsOf,
  ResultOf,
  SurqlError,
  SurqlRegistry,
} from "./registry.js";

/**
 * A cache key that remembers what it reads, borrowing TanStack Query v5's
 * `DataTag`. `client.getData(q.key)` therefore types itself with no annotation
 * and no second reference to the query.
 */
declare const dataTag: unique symbol;
export type QueryKey<T> = string & { readonly [dataTag]: T };

/**
 * The per-statement response tuple, unwrapped for the ~97% case.
 *
 * SurrealDB genuinely returns one result per statement, and that stays true:
 * `[A, B]` is preserved, `unknown[]` is preserved. Only a *single-element*
 * tuple unwraps, because a one-statement query has exactly one honest answer.
 * A scalar result stays a scalar — `Rows<[number]>` is `number`, not an array.
 */
export type Rows<R> = R extends readonly [infer Only] ? Only : R;

/** The element type of an array result. */
export type RowOf<T> = T extends ReadonlyArray<infer Element> ? Element : T;

/**
 * A named query, with its result and parameter types attached.
 *
 * `P` is the parameters still to be supplied. `.with(params)` is the only way
 * to bind them, and it returns a `SurqlQuery<R, Bound>` — "nothing remaining".
 * Adapters accept only bound queries, which gives one uniform rule the compiler
 * enforces and makes the cache key computable at the call site.
 */
export interface SurqlQuery<R, P extends Record<string, unknown> = Bound> {
  /** The exact query text, as written and as analyzed. */
  readonly text: string;
  /** Bound parameters, if any. */
  readonly params: Record<string, unknown> | undefined;
  /** Stable cache key: the text plus its stably serialised parameters. */
  readonly key: QueryKey<Rows<R>>;
  /** `false`; present so a live and a one-shot query are distinguishable. */
  readonly isLive: false;
  /** Bind this query's parameters, producing a fully bound query. */
  with(params: P): SurqlQuery<R, Bound>;
}

/**
 * A named live query. `Row` is the element type of the reconciled stream, so a
 * `LiveHandle`'s `data` is `Row[]`.
 *
 * The text is stored **without** the `LIVE` prefix — that is the text the
 * analyzer keyed, and it is also the text the SSR seed runs — while `liveText`
 * carries the prefixed form the subscription needs.
 */
export interface SurqlLive<Row, P extends Record<string, unknown> = Bound> {
  /** The bare `SELECT …` text: what the registry keys and what a seed runs. */
  readonly text: string;
  /** The `LIVE SELECT …` text the subscription opens. */
  readonly liveText: string;
  readonly params: Record<string, unknown> | undefined;
  readonly key: QueryKey<Row[]>;
  readonly isLive: true;
  with(params: P): SurqlLive<Row, Bound>;
}

/**
 * Any query reference, for positions that only need identity — `invalidate`,
 * a mutation's `invalidates` list. Structural rather than `SurqlQuery<any, any>`,
 * so the design contains no `any` at all.
 */
export interface AnyQuery {
  readonly text: string;
  readonly params: Record<string, unknown> | undefined;
  readonly key: string;
  readonly isLive: boolean;
}

/**
 * `defineQuery` always receives a literal, so a registry miss means exactly one
 * thing: the generated file is stale. That is a bug, not a mode, so it is a hard
 * type error carrying its own remedy rather than a silent degrade to `unknown`.
 * `defineQuery.unchecked(text)` is the escape hatch for a genuinely dynamic one.
 *
 * The error message is written inline rather than behind a named alias on
 * purpose: tsc prints the alias name if there is one, so `SurqlError<"…">`
 * spelled out here is what makes the remedy appear *in the compiler output*.
 *
 * This also closes a silent failure that had nothing to do with staleness:
 * reformatting a query changes its text, so it changes its key, so its result
 * quietly became `unknown[]`. Now it stops the build.
 */
export type DefinedQuery<Q extends string> = Q extends keyof SurqlRegistry
  ? SurqlQuery<ResultOf<Q>, ParamsOf<Q>>
  : SurqlError<"this query is not in the generated registry - run `surrealguard generate`">;

export type DefinedLive<Q extends string> = Q extends keyof SurqlRegistry
  ? SurqlLive<RowOf<Rows<ResultOf<Q>>>, ParamsOf<Q>>
  : SurqlError<"this query is not in the generated registry - run `surrealguard generate`">;

/** Prefix `LIVE ` unless the text already begins with it. */
function ensureLive(text: string): string {
  return /^\s*live\b/i.test(text) ? text : `LIVE ${text}`;
}

/**
 * Serialise one parameter value for the cache key.
 *
 * The SDK's own renderer is used rather than `JSON.stringify` because it
 * *distinguishes the types we care about*: a `RecordId` renders `r"team:red"`
 * where the string `"team:red"` renders `s"team:red"`. Those are different
 * queries — one matches a record link, the other cannot — so they must not
 * collide on one cache entry.
 */
function serialiseValue(value: unknown): string {
  try {
    return toSurrealqlString(value);
  } catch {
    return JSON.stringify(value) ?? String(value);
  }
}

/** Stable key for a query + params, so identical calls share a cache entry. */
export function computeKey(text: string, params?: Record<string, unknown>): string {
  if (!params) return text;
  const names = Object.keys(params).sort();
  if (names.length === 0) return text;
  const stable = names.map((name) => `${name}=${serialiseValue(params[name])}`).join("&");
  return `${text}::${stable}`;
}

function makeQuery<R>(
  text: string,
  params: Record<string, unknown> | undefined,
): SurqlQuery<R, Record<string, unknown>> {
  return {
    text,
    params,
    key: computeKey(text, params) as QueryKey<Rows<R>>,
    isLive: false,
    with: (next) => makeQuery<R>(text, { ...params, ...next }) as SurqlQuery<R, Bound>,
  };
}

function makeLive<Row>(
  text: string,
  params: Record<string, unknown> | undefined,
): SurqlLive<Row, Record<string, unknown>> {
  return {
    text,
    liveText: ensureLive(text),
    params,
    key: computeKey(text, params) as QueryKey<Row[]>,
    isLive: true,
    with: (next) => makeLive<Row>(text, { ...params, ...next }) as SurqlLive<Row, Bound>,
  };
}

/**
 * Name a one-shot query. Pass a string literal (an untagged template literal
 * works too — a *tagged* one does not, because TypeScript widens a tagged
 * template's cooked text to `string`; the TS#33304 limit is why gql.tada uses
 * the same call form).
 *
 * ```ts
 * const peopleOf = defineQuery("SELECT id, name FROM person WHERE team = $team");
 * const rows = await db.run(peopleOf, { team: new RecordId("team", "red") });
 * ```
 */
export function defineQuery<Q extends string>(text: Q): DefinedQuery<Q> {
  return makeQuery(text, undefined) as unknown as DefinedQuery<Q>;
}

/**
 * Name a live query. Write it **without** the `LIVE` prefix — the same text the
 * analyzer sees — and the prefix is added for the subscription.
 */
export function defineLive<Q extends string>(text: Q): DefinedLive<Q> {
  return makeLive(text, undefined) as unknown as DefinedLive<Q>;
}

/**
 * Name a query the generated registry does not (and will not) contain. The
 * result degrades to `unknown[]` — never `any` — and parameters become
 * optional. Use it for a genuinely dynamic query; for a stale generated file,
 * regenerate instead.
 */
defineQuery.unchecked = (text: string): SurqlQuery<unknown[], Record<string, unknown>> =>
  makeQuery<unknown[]>(text, undefined);

/** {@link defineQuery.unchecked}, for a live query. */
defineLive.unchecked = (text: string): SurqlLive<unknown, Record<string, unknown>> =>
  makeLive<unknown>(text, undefined);
