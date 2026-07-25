/**
 * The typed `db.live` contract.
 *
 * `db.live(\`SELECT * FROM user\`)` resolves a {@link LiveDescriptor} — a plain
 * object carrying the `LIVE SELECT …` text and, at the type level only, the row
 * shape. The row is inferred from the generated {@link SurqlRegistry} with
 * exactly the rigor of `db.query`: a registered query is fully typed, a
 * non-registered one degrades to `unknown` (never `any`).
 *
 * The query is passed as a string / template-literal **argument** (note the
 * parentheses), not a *tagged* template. TypeScript widens a tagged template's
 * cooked text to `string` (issue #33304), so the literal — and thus the row
 * type — would be lost. An untagged template literal keeps its literal type, so
 * `db.live(\`…\`)` and `db.live("…")` are both fully typed. A bare tagged call
 * `db.live\`…\`` still runs, but its row degrades to `unknown`.
 *
 * The framework adapters (`@surrealguard/svelte`, `@surrealguard/next`) consume
 * the descriptor: they read `.sql` at runtime and carry `Row` at the type level.
 */

import type { SurqlRegistry } from "./registry.js";

/**
 * A live query, produced by `db.live\`…\``. `sql` is the `LIVE SELECT …` text
 * ready for the reactive core; `Row` is the (phantom) element type of each
 * emitted row. The `__row` marker never exists at runtime — it exists only so
 * the descriptor carries `Row` through the type system to the adapters.
 */
export interface LiveDescriptor<Row = unknown> {
  /** The `LIVE SELECT …` text, `LIVE`-prefixed and ready for the core. */
  readonly sql: string;
  /** Optional bindings captured with the query. */
  readonly params?: Record<string, unknown>;
  /** Phantom carrier for `Row`; never present at runtime. */
  readonly __row?: (row: Row) => void;
}

/** The element type of an array result (results are `Array<Row>` shapes). */
export type RowOf<T> = T extends ReadonlyArray<infer E> ? E : T;

/**
 * A registry `result` is the per-statement response tuple (one element per
 * statement). A live query is a single statement, so its live row comes from
 * that one element's `Array<Row>`. Unwrap the first tuple element; a plain
 * (non-tuple) result passes through so either shape resolves.
 */
type FirstStatement<T> = T extends readonly [infer First, ...unknown[]] ? First : T;

/**
 * Resolve a live query's row type from the registry. The generator keys a live
 * query by its analyzed text, which is `LIVE`-prefixed; the tagged template is
 * written without the prefix. So the `LIVE `-prefixed key is tried first, then
 * the bare text (in case a registry keys it either way), and otherwise the row
 * degrades to `unknown` — the same non-`any` fallback `db.query` uses.
 */
export type LiveRowOf<Q extends string> = `LIVE ${Q}` extends keyof SurqlRegistry
  ? RowOf<FirstStatement<SurqlRegistry[`LIVE ${Q}`]["result"]>>
  : Q extends keyof SurqlRegistry
    ? RowOf<FirstStatement<SurqlRegistry[Q]["result"]>>
    : unknown;

/** Prefix `LIVE ` unless the text already begins with it. */
export function ensureLive(text: string): string {
  return /^\s*live\b/i.test(text) ? text : `LIVE ${text}`;
}

/** Build the `LIVE SELECT …` text from tagged-template parts + interpolations. */
export function buildLiveSql(strings: readonly string[], values: readonly unknown[]): string {
  let text = "";
  for (let i = 0; i < strings.length; i += 1) {
    text += strings[i] ?? "";
    if (i < values.length) text += String(values[i]);
  }
  return ensureLive(text);
}
