/**
 * Type-level tests for the `db.live` contract. Compiling this file IS the test:
 * positive lines must typecheck and every `@ts-expect-error` line must fail.
 *
 * Mirrors the rigor of `query.test-d.ts`: a registered live query resolves a
 * fully typed row; a non-registered one degrades to `unknown` (never `any`).
 *
 * Pure type checks — `main` is never called, so no connection opens.
 */

import { SurrealGuardClient, type LiveDescriptor, type RecordId } from "../src/index.js";

// Stand in for what `surrealguard generate` emits. The generator keys a live
// query by its analyzed (`LIVE`-prefixed) text; a plain SELECT keeps its text.
declare module "../src/registry.js" {
  interface SurqlRegistry {
    "LIVE SELECT * FROM user": {
      result: [Array<{ id: RecordId<"user">; name: string; age: number }>];
      params: Record<string, never>;
    };
    "SELECT name FROM user": {
      result: [Array<{ name: string }>];
      params: Record<string, never>;
    };
  }
}

/** True iff `T` is exactly `any`. */
type IsAny<T> = 0 extends 1 & T ? true : false;
/** Exact type equality (invariant), for asserting an inferred row shape. */
type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;
type Expect<T extends true> = T;
/** Extract the phantom `Row` a descriptor carries. */
type RowOfDescriptor<D> = D extends LiveDescriptor<infer R> ? R : never;

const db = new SurrealGuardClient();

async function main() {
  // A registered live query (keyed `LIVE`-prefixed in the registry) resolves the
  // fully typed row, written WITHOUT the `LIVE` prefix. The query is passed as a
  // template-literal ARGUMENT (parentheses) so its literal type survives.
  const users = db.live(`SELECT * FROM user`);
  type UserRow = RowOfDescriptor<typeof users>;
  type _users = Expect<Equal<UserRow, { id: RecordId<"user">; name: string; age: number }>>;

  // A registry that keys the bare SELECT text also resolves — proving the
  // prefix-agnostic lookup. `Row = { name: string }`. Plain string works too.
  const named = db.live("SELECT name FROM user");
  type NamedRow = RowOfDescriptor<typeof named>;
  type _named = Expect<Equal<NamedRow, { name: string }>>;

  // A non-registered live query degrades to `unknown` — NOT `any`.
  const dynamic = db.live(`SELECT * FROM nonexistent`);
  type DynamicRow = RowOfDescriptor<typeof dynamic>;
  type _notAny = Expect<Equal<IsAny<DynamicRow>, false>>;
  type _unknown = Expect<Equal<DynamicRow, unknown>>;

  // The descriptor carries the `LIVE SELECT …` text at runtime.
  const sql: string = users.sql;
  void sql;

  // A bare *tagged* template still compiles (runs), but its row is `unknown` —
  // TypeScript cannot recover a tagged template's cooked text as a literal.
  const tagged = db.live`SELECT * FROM user`;
  type TaggedRow = RowOfDescriptor<typeof tagged>;
  type _tagged = Expect<Equal<TaggedRow, unknown>>;

  // --- negatives: each must fail to compile ---
  // @ts-expect-error a degraded (`unknown`) row is not assignable to a shape
  const bad: { id: string } = null as unknown as DynamicRow;
  void bad;

  void users;
  void named;
  void dynamic;
  void tagged;
}
void main;
