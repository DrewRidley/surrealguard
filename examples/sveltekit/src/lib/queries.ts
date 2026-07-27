// Named queries — for when two places need to agree on one query.
//
// `db.query("SELECT …")` is already fully typed on its own; `src/routes/+page.svelte`
// uses nothing but that. These names exist because `src/routes/live/` has a
// `+page.ts` that fetches on the server and a `+page.svelte` that subscribes on
// the client, and they must mean the same query.
//
// In 0.4 they each spelled `SELECT name, age, team FROM person` out. Change one
// and the cache key stopped matching, so the SSR seed was silently discarded
// and the page refetched — with no error, no warning, and no type failure. A
// query value written once cannot drift.
//
// `defineQuery` / `defineLive` read the same registry through the same
// conditional generic as `db.query`, so they add no type safety of their own.
// What they add is identity: `db.run` (which unwraps a single-statement
// result), `db.watch`, `db.invalidate` and the reactive helpers all take a
// query reference rather than a string.

import { defineLive, defineQuery } from "$lib/surrealguard.generated";

export const allPeople = defineQuery("SELECT id, name, age, team FROM person");
export const peopleOf = defineQuery("SELECT id, name FROM person WHERE team = $team");
export const addPerson = defineQuery(
  "CREATE person SET name = $name, age = $age, team = $team",
);

export const livePeople = defineLive("SELECT id, name, age, team FROM person");
export const liveTeam = defineLive("SELECT id, name FROM person WHERE team = $team");
