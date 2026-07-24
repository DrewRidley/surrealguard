/**
 * Type-level tests for the Next adapter. Compiling this file (`tsc --noEmit`) IS
 * the test: `useLiveQuery(...).data` must be exactly `Row[]` for a registered
 * live query, and `unknown[]` (never `any[]`) for a non-registered one —
 * mirroring the `db.query` / `db.live` rigor.
 *
 * `_typeCheck` is never called; the React hooks inside `useLiveQuery` don't run.
 */

import { useLiveQuery } from "../src/index.js";
import { queryServer } from "../src/server.js";
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

async function _typeCheck(client: SurrealGuardClient) {
  // A registered live query: `data` is fully typed `{ name: string }[]`.
  const state = useLiveQuery((db) => db.live(`SELECT name FROM user`), { client });
  type Data = typeof state.data;
  type _data = Expect<Equal<Data, { name: string }[]>>;
  const s: "loading" | "success" | "error" = state.status;
  const e: unknown = state.error;
  void s;
  void e;

  // A non-registered live query degrades to `unknown[]` — NOT `any[]`.
  const dyn = useLiveQuery((db) => db.live(`SELECT x FROM y`), { client });
  type DynData = typeof dyn.data;
  type _dynUnknown = Expect<Equal<DynData, unknown[]>>;
  type _dynNotAny = Expect<Equal<IsAny<DynData>, false>>;

  // The server seed helper carries the same typed row.
  const seeded = await queryServer(client, client.live(`SELECT name FROM user`));
  type Seeded = typeof seeded;
  type _seeded = Expect<Equal<Seeded, { name: string }[]>>;

  type _all = [_data, _dynUnknown, _dynNotAny, _seeded];
  return null as unknown as _all;
}
void _typeCheck;
