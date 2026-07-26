// The one place query text lives.
//
// `defineQuery` / `defineLive` infer the string literal and resolve its result
// and parameter types from the generated registry — exactly as `db.query` does,
// through the same single conditional generic and with no permissive `string`
// overload. The difference is that the literal is written ONCE, here, instead
// of once per consumer, so an SSR seed and a component cannot drift apart
// byte-for-byte and silently miss the cache.
//
// A query text the registry does not contain is a compile error carrying its
// own remedy, so a stale generated file stops the build instead of quietly
// degrading the result to `unknown[]`.

import { defineLive, defineQuery } from "../surrealguard.generated";

export const allPeople = defineQuery("SELECT id, name, age, team FROM person");
export const peopleOf = defineQuery("SELECT id, name FROM person WHERE team = $team");
export const namesAndAges = defineQuery("SELECT name FROM person; SELECT age FROM person");
export const addPerson = defineQuery(
  "CREATE person SET name = $name, age = $age, team = $team",
);

export const livePeople = defineLive("SELECT id, name, age, team FROM person");
export const liveTeam = defineLive("SELECT id, name FROM person WHERE team = $team");
