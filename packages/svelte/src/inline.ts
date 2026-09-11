/**
 * The runtime behind the inline query attribute.
 *
 * ```svelte
 * <Query q="SELECT id, name, age FROM person WHERE age > {minAge}">
 * ```
 *
 * The preprocessor in `./preprocess.js` rewrites that, before Svelte compiles
 * it, into a call to {@link sgQuery} below:
 *
 * ```js
 * q={() => __sg_query(["SELECT id, name, age FROM person WHERE age > ", ""], [minAge])}
 * ```
 *
 * which produces the query
 *
 *   text:   "SELECT id, name, age FROM person WHERE age > $__host0"
 *   params: { __host0: minAge }
 *
 * Three things about that shape, each of which is the reason it exists rather
 * than a detail of it:
 *
 * 1. **The value never enters the text.** Svelte would otherwise compile the
 *    attribute to string concatenation, so `minAge` would be spliced in before
 *    anything here could see it. A string containing a quote would then be a
 *    SurrealQL injection, and a number would produce a different query text on
 *    every keystroke — a cache miss per frame, and an unbounded cache.
 * 2. **The static skeleton is what the registry is keyed by,** so
 *    `SELECT … > $__host0` is one entry no matter what `minAge` is, and it is
 *    what SurrealQL Analyzer analyses. The result type is looked up from it here,
 *    which is why the snippet parameter is typed with no annotation at the call
 *    site.
 * 3. **The parameter names are positional and deterministic** — `$__host0`,
 *    `$__host1`, … left to right, zero-indexed. `__host` rather than something
 *    readable for two reasons: `crates/embed` already spells a host
 *    substitution that way on the TypeScript path, and a name a user might
 *    plausibly choose (`$p0`) would silently collide with a parameter they had
 *    written themselves. The analyzer and this runtime must agree on the text
 *    byte for byte or the registry lookup misses, so {@link HOST_PARAM_PREFIX}
 *    is the single place it is spelled.
 */

import {
  defineLive,
  defineQuery,
  type Bound,
  type ResultOf,
  type RowOf,
  type Rows,
  type SurqlLive,
  type SurqlQuery,
} from "@surrealdb/analyzer-client";

/**
 * The one place the parameter prefix is written. The type-level {@link Skeleton}
 * cannot read a `const`, so its literal spelling is duplicated there and only
 * there — and `test/inline.test.ts` asserts the two agree.
 */
export const HOST_PARAM_PREFIX = "__host";

/** `[0,1,2,…]` — a counter for {@link Skeleton}, which caps interpolations at 24. */
type Increment = [
  1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
];

/**
 * The query text the parts add up to: the static strings, joined by
 * `$__host0`, `$__host1`, … in source order.
 *
 * `["SELECT … > ", ""]` becomes `"SELECT … > $__host0"`, which is a string
 * *literal* type, and that is what makes the registry lookup below possible at
 * all. The prefix is spelled out because a type cannot read
 * {@link HOST_PARAM_PREFIX}; keep the two in step.
 */
export type Skeleton<
  Parts extends readonly string[],
  Index extends number = 0,
> = Parts extends readonly [infer Only extends string]
  ? Only
  : Parts extends readonly [infer Head extends string, ...infer Tail extends readonly string[]]
    ? Index extends keyof Increment
      ? `${Head}$__host${Index}${Skeleton<Tail, Increment[Index]>}`
      : string
    : "";

/**
 * Build a one-shot query from an interpolated attribute's parts.
 *
 * Emitted by the preprocessor; there is no reason to call it by hand, and the
 * leading underscores in the name it is imported under say so.
 */
export function sgQuery<const Parts extends readonly string[]>(
  parts: Parts,
  values: readonly unknown[],
): SurqlQuery<ResultOf<Skeleton<Parts>>, Bound> {
  const { text, params } = assemble(parts, values);
  const query = defineQuery.unchecked(text);
  // No `.with({})` when nothing was interpolated: an empty binding would give
  // the entry the key `text::` rather than `text`, and `invalidate` matches
  // bindings by prefix off the bare text.
  return (params ? query.with(params) : query) as unknown as SurqlQuery<
    ResultOf<Skeleton<Parts>>,
    Bound
  >;
}

/** {@link sgQuery}, for `<LiveQuery>`. */
export function sgLive<const Parts extends readonly string[]>(
  parts: Parts,
  values: readonly unknown[],
): SurqlLive<RowOf<Rows<ResultOf<Skeleton<Parts>>>>, Bound> {
  const { text, params } = assemble(parts, values);
  const live = defineLive.unchecked(text);
  return (params ? live.with(params) : live) as unknown as SurqlLive<
    RowOf<Rows<ResultOf<Skeleton<Parts>>>>,
    Bound
  >;
}

/**
 * Build a query from an attribute that is a plain string, binding `params` if
 * there are any: `<Query q="SELECT … WHERE age > $min" params={{ min }} />`.
 *
 * This form needs no preprocessor at all, and that is the point of it. The
 * attribute stays a string *literal*, so TypeScript keeps its literal type and
 * the registry lookup that types the snippet works in the editor — where the
 * interpolated form's type is lost, because `svelte2tsx` applies `script` and
 * `style` preprocessors but not `markup` ones. It is also plainer SurrealQL:
 * `$min` is a parameter the language already has, so what you read in the
 * attribute is what the database is sent.
 */
export function sgText(
  text: string,
  params: Record<string, unknown> | undefined,
): SurqlQuery<unknown, Bound> {
  const query = defineQuery.unchecked(text);
  // Bind nothing rather than `{}`: an empty binding keys the entry `text::`
  // instead of `text`, and `invalidate` matches by prefix off the bare text.
  return (params ? query.with(params) : query) as SurqlQuery<unknown, Bound>;
}

/** {@link sgText}, for `<LiveQuery>`. */
export function sgTextLive(
  text: string,
  params: Record<string, unknown> | undefined,
): SurqlLive<unknown, Bound> {
  const live = defineLive.unchecked(text);
  return (params ? live.with(params) : live) as SurqlLive<unknown, Bound>;
}

/**
 * The runtime half of {@link Skeleton}. Kept next to it, because the two
 * agreeing is the whole contract: a mismatch is a silent registry miss.
 */
function assemble(
  parts: readonly string[],
  values: readonly unknown[],
): { text: string; params: Record<string, unknown> | undefined } {
  if (values.length === 0) return { text: parts.join(""), params: undefined };
  const params: Record<string, unknown> = {};
  let text = parts[0] ?? "";
  for (let index = 0; index < values.length; index += 1) {
    params[`${HOST_PARAM_PREFIX}${index}`] = values[index];
    text += `$${HOST_PARAM_PREFIX}${index}${parts[index + 1] ?? ""}`;
  }
  return { text, params };
}
