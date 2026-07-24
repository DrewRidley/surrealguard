// A vanilla-TypeScript SurrealGuard demo.
//
// `surrealguard generate` scanned this file, found the `db.query("...")` calls
// below, analyzed each against `schema/schema.surql`, and wrote
// `surrealguard.generated.ts` — a module augmentation that types every query by
// its exact text. We import the client from that generated file, so the
// augmentation loads with it and `db.query(...)` is fully typed.

import { SurrealGuardClient } from "../surrealguard.generated";
import type { RecordId } from "@surrealguard/client";

async function main() {
  const db = new SurrealGuardClient();
  await db.connect("ws://localhost:8000/rpc");

  const team = "team:red" as RecordId<"team">;

  // Result AND params are inferred from the query text against the schema:
  //   result: Array<{ name: string }>
  //   params: { team: RecordId<"team"> }   (person.team is `record<team>`)
  const [rows] = await db.query(
    "SELECT name FROM person WHERE team = $team",
    { team },
  );

  for (const row of rows) {
    // `row.name` is a string; `row.nope` would be a compile error.
    console.log(row.name.toUpperCase());
  }

  // A second query with no params: the params argument is forbidden, and the
  // result carries the record link as a branded `RecordId<"team">`.
  const [people] = await db.query("SELECT name, age, team FROM person");
  for (const person of people) {
    console.log(person.name, person.age, person.team);
  }

  // ---- The guarantee, made concrete ---------------------------------------
  // A wrong param type is a *compile* error, not a runtime surprise. `team` is
  // a `RecordId<"team">` (a branded string); a number is rejected. The
  // `@ts-expect-error` asserts tsc catches it — remove it and `tsc --noEmit`
  // fails, which is the whole point of generating types.
  // @ts-expect-error team must be a RecordId<"team">, not a number.
  await db.query("SELECT name FROM person WHERE team = $team", { team: 123 });
}

void main;
