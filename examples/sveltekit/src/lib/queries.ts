// The one place query text lives.
//
// This is the fix for the structural flaw the redesign exists to address: in
// 0.4, `+page.ts` and `+page.svelte` each spelled out
// `SELECT name, age, team FROM person`. Change one and the cache key stopped
// matching, so the SSR seed was silently discarded and the page refetched —
// with no error, no warning, and no type failure.

import { defineLive, defineQuery } from "../../surrealguard.generated";

export const allPeople = defineQuery("SELECT id, name, age, team FROM person");
export const peopleOf = defineQuery("SELECT id, name FROM person WHERE team = $team");
export const addPerson = defineQuery(
  "CREATE person SET name = $name, age = $age, team = $team",
);

export const livePeople = defineLive("SELECT id, name, age, team FROM person");
export const liveTeam = defineLive("SELECT id, name FROM person WHERE team = $team");
