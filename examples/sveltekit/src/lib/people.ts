// The typed data layer, exercised so `tsc --noEmit` proves the registry types
// are real. These functions are what a component calls; the runes machinery
// lives inside `@surrealguard/svelte`'s `liveQuery`, so nothing here needs a
// `.svelte` file to type-check.

import { liveQuery, type LiveQuery } from "@surrealguard/svelte";
import { db } from "$lib/db";

interface Person {
  name: string;
  age: number;
}

/**
 * A runes-reactive live view of `person`. The row type flows from the generated
 * registry through `db.live(...)`, so `.data` is `Array<{ name; age; team; ...}>`
 * with no cast. (Called from a component's script; not executed at import.)
 */
export function livePeople(): LiveQuery<Person> {
  return liveQuery((client) =>
    client.live(`SELECT name, age, team FROM person`),
  );
}

/** A typed one-shot query: params required, result typed — both from the text. */
export async function personNames(team: import("@surrealguard/client").RecordId<"team">) {
  const [rows] = await db.query(
    "SELECT name FROM person WHERE team = $team",
    { team },
  );
  return rows.map((row) => row.name); // row.name is `string`
}

// ---- The guarantee, made concrete -----------------------------------------
// A wrong param type is a *compile* error. `team` is a branded RecordId, so a
// number is rejected. The `@ts-expect-error` asserts tsc catches it — delete it
// and `tsc --noEmit` fails, which proves the types are load-bearing.
export async function wrongParamIsRejected() {
  // @ts-expect-error team must be a RecordId<"team">, not a number.
  await db.query("SELECT name FROM person WHERE team = $team", { team: 123 });
}
