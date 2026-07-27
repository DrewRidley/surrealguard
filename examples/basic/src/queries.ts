// Named queries — optional, and worth it here.
//
// `db.query("SELECT …")` is already fully typed on its own; see `main.ts`.
// `defineQuery` / `defineLive` infer the same string literal and read the same
// registry through the same conditional generic, so they buy no extra type
// safety. What they buy is a VALUE:
//
//   - the text is written ONCE, so two consumers cannot drift apart
//     byte-for-byte and silently miss the cache;
//   - `db.run` unwraps a single-statement result, so there is no destructure;
//   - `db.watch` and `db.invalidate` need a reference to hold on to.
//
// A query text the registry does not contain is a compile error carrying its
// own remedy, so a stale generated file stops the build instead of quietly
// degrading the result to `unknown[]`.

import { defineLive, defineQuery } from "./surrealguard.generated";

export const allPeople = defineQuery("SELECT id, name, age, team FROM person");
export const peopleOf = defineQuery("SELECT id, name FROM person WHERE team = $team");
export const namesAndAges = defineQuery("SELECT name FROM person; SELECT age FROM person");
export const addPerson = defineQuery(
  "CREATE person SET name = $name, age = $age, team = $team",
);

export const livePeople = defineLive("SELECT id, name, age, team FROM person");
export const liveTeam = defineLive("SELECT id, name FROM person WHERE team = $team");
