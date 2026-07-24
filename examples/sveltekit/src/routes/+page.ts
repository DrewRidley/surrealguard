// SvelteKit `load`: seed the live query's rows on the server so the first paint
// is gap-free, then the component upgrades to live on the client.
//
// `loadLive` runs the live descriptor's underlying SELECT once; its row type is
// inferred from the generated registry, so `people` is fully typed here.

import { loadLive } from "@surrealguard/svelte";
import { db } from "$lib/db";

export async function load() {
  const people = await loadLive(
    db,
    db.live(`SELECT name, age, team FROM person`),
  );

  // `people` is Array<{ name: string; age: number; team: RecordId<"team">; ... }>
  // — typed from the schema, no cast. Reading a bogus field is a compile error.
  const names: string[] = people.map((person) => person.name);

  return { people, names };
}
