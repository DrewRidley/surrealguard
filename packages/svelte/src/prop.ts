/**
 * What `<Query>` / `<LiveQuery>` accept as `q`, and the rows that follow from it.
 *
 * Both components take a single type parameter, inferred straight off the `q`
 * prop, and read the row type back out of it here. That shape is deliberate: an
 * earlier version carried a second parameter for the value form and chose
 * between the two with a conditional type, but a conditional is not an inference
 * site, so nothing ever bound it and every snippet parameter degraded to
 * `unknown` — the exact annotation-at-the-call-site this exists to remove.
 *
 * The text form is constrained to `keyof SurqlRegistry`, so a query the
 * generated registry does not know is a type error **on the attribute**, where
 * the mistake is, rather than a silent `unknown` downstream. Reformatting a
 * query changes its text and therefore its key, so this is the check that
 * catches an edit nobody thinks of as one — `surrealguard generate` picks the
 * new text up on its next run, and until it does, the build stops.
 */

import type {
  Bound,
  Json,
  Preloaded,
  ResultOf,
  Rows,
  SurqlLive,
  SurqlQuery,
  SurqlRegistry,
} from "@surrealguard/client";

/**
 * A one-shot query value. `Bound` — not `any` — is the parameter type on
 * purpose: a query still carrying unbound parameters is not renderable, and
 * saying so here makes `<Query q={membersOf}>` a type error pointing at the
 * missing `.with({…})` rather than a request that fails at runtime. `SurqlLive`
 * is excluded structurally, by `isLive`, so a live query sent to `<Query>` is
 * caught too.
 */
type OneShotValue = SurqlQuery<any, Bound> | Preloaded<any>;

/** The same, for a subscription. */
type LiveValue = SurqlLive<any, Bound> | Preloaded<any>;

/**
 * Anything `<Query>`'s `q` accepts: the text of a registered query, a bound
 * query value, a thunk returning either (which is what keeps parameters
 * reactive), or `"skip"`.
 */
export type QuerySource =
  | (keyof SurqlRegistry & string)
  | OneShotValue
  | "skip"
  | (() => OneShotValue | "skip");

/** Anything `<LiveQuery>`'s `q` accepts. */
export type LiveSource =
  | (keyof SurqlRegistry & string)
  | LiveValue
  | "skip"
  | (() => LiveValue | "skip");

/**
 * The per-statement result tuple `q` resolves to.
 *
 * A conditional type distributes over a union, so the `"skip"` a thunk may
 * return has to be dropped *before* the rest is resolved: without the
 * `Exclude`, `() => Live | "skip"` answers `Row | unknown`, which collapses to
 * `unknown` and silently un-types the snippet.
 */
export type QueryResult<Q> = Q extends "skip"
  ? unknown
  : Q extends string
    ? ResultOf<Q>
    : Q extends SurqlQuery<infer R, any>
      ? R
      : Q extends Preloaded<infer D>
        ? D
        : Q extends () => infer T
          ? QueryResult<Exclude<T, "skip">>
          : unknown;

/** The row type a `<LiveQuery>` streams: its result, unwrapped one level. */
export type LiveRow<Q> = Q extends "skip"
  ? unknown
  : Q extends SurqlLive<infer Row, any>
    ? Row
    : Q extends () => infer T
      ? LiveRow<Exclude<T, "skip">>
      : Q extends Preloaded<Array<infer Row>>
        ? Row
        : RowOfRows<Rows<QueryResult<Q>>>;

/** `Person[]` → `Person`; anything else is already a row. */
type RowOfRows<T> = T extends readonly (infer Row)[] ? Row : T;

/** What a `<Query>` hands its `children` snippet. */
export type QueryRows<Q> = Json<Rows<QueryResult<Q>>>;

/** What a `<LiveQuery>` hands its `children` snippet: always an array. */
export type LiveRows<Q> = Json<LiveRow<Q>>[];

/** The bound form the components hand to the reactive core. */
export type BoundQuery<Q> = SurqlQuery<QueryResult<Q>, Bound>;
