/**
 * `@surrealdb/analyzer-svelte/transport` — a SvelteKit `transport` hook for the SDK's
 * value classes.
 *
 * You do not need this by default. The reactive layer and `preload` are
 * `Json<T>`-shaped precisely so that devalue never meets a class instance.
 *
 * You need it when you want SDK-class fidelity through `load` — returning a
 * `RecordId` or a `Duration` from a `load` function, or a `db.run` result
 * directly. devalue rejects arbitrary class instances, so without this the
 * value never reaches the browser. 0.4 offered no option at all.
 *
 * ```ts
 * // src/hooks.ts
 * export { transport } from "@surrealdb/analyzer-svelte/transport";
 * ```
 *
 * To add your own entries, spread it:
 * ```ts
 * import { transport as surrealqlAnalyzer } from "@surrealdb/analyzer-svelte/transport";
 * export const transport = { ...surrealqlAnalyzer, MyType: { encode, decode } };
 * ```
 */

import { Decimal, DateTime, Duration, RecordId, Uuid } from "surrealdb";
import type { RecordIdValue } from "surrealdb";

/**
 * SvelteKit's contract: `encode` returns `false` for a value it does not
 * handle, and anything else is the serialisable payload handed back to
 * `decode`. `Raw` is that payload's own shape.
 */
interface Transporter<T, Raw> {
  encode(value: unknown): false | Raw;
  decode(raw: Raw): T;
}

export const transport = {
  RecordId: {
    encode: (value) => value instanceof RecordId && [String(value.table), value.id],
    decode: ([table, id]) => new RecordId(table, id),
  } satisfies Transporter<RecordId, [string, RecordIdValue]>,

  DateTime: {
    encode: (value) => value instanceof DateTime && value.toString(),
    decode: (iso) => new DateTime(iso),
  } satisfies Transporter<DateTime, string>,

  Duration: {
    encode: (value) => value instanceof Duration && value.toString(),
    decode: (text) => new Duration(text),
  } satisfies Transporter<Duration, string>,

  Uuid: {
    encode: (value) => value instanceof Uuid && value.toString(),
    decode: (text) => new Uuid(text),
  } satisfies Transporter<Uuid, string>,

  Decimal: {
    encode: (value) => value instanceof Decimal && value.toString(),
    decode: (text) => new Decimal(text),
  } satisfies Transporter<Decimal, string>,
};
