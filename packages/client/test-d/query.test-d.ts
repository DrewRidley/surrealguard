/**
 * Type-level tests for the `db.query` contract. Compiling this file IS the
 * test: the positive lines must typecheck, and every `@ts-expect-error` line
 * must fail (tsc errors if an expected-error line unexpectedly compiles).
 *
 * These are pure type checks — `main` is never called, so no connection opens.
 */

import { SurrealGuardClient, type RecordId } from "../src/index.js";

// Stand in for what `surrealguard generate` emits. Each `result` is the
// per-statement response tuple: one element per statement, `null` for a
// non-responder. A single-statement query is a one-element tuple.
declare module "../src/registry.js" {
  interface SurqlRegistry {
    "SELECT * FROM user": {
      result: [Array<{ id: RecordId<"user">; name: string; age: number }>];
      params: Record<string, never>;
    };
    "SELECT * FROM user WHERE team = $team": {
      result: [Array<{ id: RecordId<"user">; name: string }>];
      params: { team: string };
    };
    "SELECT name FROM user; SELECT age FROM user": {
      result: [Array<{ name: string }>, Array<{ age: number }>];
      params: Record<string, never>;
    };
  }
}

const db = new SurrealGuardClient();

async function main() {
  // Result type is inferred; SurrealDB returns one result per statement, so
  // destructure the first statement's rows. No params on a param-free query.
  const [users] = await db.query("SELECT * FROM user");
  users[0]!.name.toUpperCase();
  users[0]!.age.toFixed(0);

  // Params required and typed from the query text.
  const [team] = await db.query("SELECT * FROM user WHERE team = $team", { team: "red" });
  team[0]!.name.length;

  // Multi-statement: one result per statement, in order. Each destructured
  // element is typed from its own statement.
  const [names, ages] = await db.query("SELECT name FROM user; SELECT age FROM user");
  names[0]!.name.toUpperCase();
  ages[0]!.age.toFixed(0);
  // @ts-expect-error the tuple has exactly two statements — no third element
  const [, , third] = await db.query("SELECT name FROM user; SELECT age FROM user");
  void third;

  // The SDK's fluent builder is still available on a typed query. Chaining a
  // builder method resolves to the SDK's own `unknown[]` (its generic default);
  // the narrowed result comes from awaiting `db.query(...)` directly, above.
  const rows: unknown[] = await db.query("SELECT * FROM user").retry();
  void rows;

  // Dynamic strings fall back to the SDK's `unknown[]`.
  const dynamic: string = "SELECT " + "1";
  const anyResult = await db.query(dynamic);
  void anyResult;

  // `SurrealGuardClient` is a real `Surreal`: SDK methods are present.
  await db.connect("ws://localhost:8000/rpc");
  await db.use({ namespace: "test", database: "test" });

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
