/**
 * Type-level tests for the Svelte adapter. Type-checking this file (via
 * `svelte-check`) IS the test: `users.data` must be exactly `Row[]` for a
 * registered live query, and `unknown[]` (never `any[]`) for a non-registered
 * one — mirroring the `db.query` / `db.live` rigor.
 *
 * `_typeCheck` is never called; the runes inside `liveQuery` don't run here.
 */

import { liveQuery } from "../src/index.js";
import { SurrealGuardClient } from "@surrealguard/client";

// Stand in for what `surrealguard generate` emits for a live query.
declare module "@surrealguard/client" {
  interface SurqlRegistry {
    "LIVE SELECT name FROM user": {
      result: Array<{ name: string }>;
      params: Record<string, never>;
    };
  }
}

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;
type Expect<T extends true> = T;
type IsAny<T> = 0 extends 1 & T ? true : false;

function _typeCheck(client: SurrealGuardClient) {
  // A registered live query: `users.data` is fully typed `{ name: string }[]`.
  const users = liveQuery((db) => db.live(`SELECT name FROM user`), { client });
  type Data = typeof users.data;
  type _data = Expect<Equal<Data, { name: string }[]>>;

  // status/loading/error are present and typed.
  const s: "loading" | "success" | "error" = users.status;
  const l: boolean = users.loading;
  const e: unknown = users.error;
  void s;
  void l;
  void e;

  // A non-registered live query degrades to `unknown[]` — NOT `any[]`.
  const dyn = liveQuery((db) => db.live(`SELECT x FROM y`), { client });
  type DynData = typeof dyn.data;
  type _dynUnknown = Expect<Equal<DynData, unknown[]>>;
  type _dynNotAny = Expect<Equal<IsAny<DynData>, false>>;

  // Keep the assertions live (types are only checked when referenced).
  type _all = [_data, _dynUnknown, _dynNotAny];
  return null as unknown as _all;
}
void _typeCheck;
