/**
 * Type-level tests for the `db.query` contract. Compiling this file IS the
 * test: the positive lines must typecheck, and every `@ts-expect-error` line
 * must fail (tsc errors if an expected-error line unexpectedly compiles).
 */

import { SurrealGuardClient, type Connection, type RecordId } from "../src/index.js";

// Stand in for what `surrealguard generate` emits.
declare module "../src/registry.js" {
  interface SurqlRegistry {
    "SELECT * FROM user": {
      result: Array<{ id: RecordId<"user">; name: string; age: number }>;
      params: Record<string, never>;
    };
    "SELECT * FROM user WHERE team = $team": {
      result: Array<{ id: RecordId<"user">; name: string }>;
      params: { team: string };
    };
  }
}

declare const conn: Connection;
const db = new SurrealGuardClient(conn);

async function main() {
  // Result type is inferred; no params allowed on a param-free query.
  const users = await db.query("SELECT * FROM user");
  users[0]!.name.toUpperCase();
  users[0]!.age.toFixed(0);

  // Params required and typed from the query text.
  const team = await db.query("SELECT * FROM user WHERE team = $team", { team: "red" });
  team[0]!.name.length;

  // Dynamic strings fall back to `unknown`.
  const dynamic: string = "SELECT " + "1";
  const anyResult = await db.query(dynamic);
  void anyResult;

  // --- negatives: each must fail to compile ---
  // @ts-expect-error missing required params
  await db.query("SELECT * FROM user WHERE team = $team");
  // @ts-expect-error wrong param type
  await db.query("SELECT * FROM user WHERE team = $team", { team: 123 });
  // @ts-expect-error params passed to a no-param query
  await db.query("SELECT * FROM user", { team: "red" });
  // @ts-expect-error unknown result field
  (await db.query("SELECT * FROM user"))[0]!.nonexistent;
}
void main;
