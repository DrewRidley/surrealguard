// Named queries — for when two places need to agree on one query.
//
// `db.query("SELECT …")` is already fully typed on its own; `src/routes/+page.svelte`
// uses nothing but that. These names exist because the demo page passes query
// VALUES around: `<Query q={peopleOver.with({ minAge })}>`, `createMutation(addPerson,
// { invalidates: [allPeople, allTeams] })`, `preload(db, livePeople)`. A string
// cannot be a value, and a live query needs a reference to re-subscribe with.
//
// `defineQuery` / `defineLive` read the same registry through the same
// conditional generic as `db.query`, so they add no type safety of their own.
// What they add is identity: `db.run` (which unwraps a single-statement
// result), `db.watch`, `db.invalidate` and the reactive helpers all take a
// query reference rather than a string.

import { defineLive, defineQuery } from "$lib/surrealguard.generated";

// ---------------------------------------------------------------- people ---

export const allPeople = defineQuery("SELECT id, name, age, team FROM person");

/**
 * Beat 1, parameter tracking. `$minAge` is what the slider on the demo page
 * moves; passing it through a thunk — `() => peopleOver.with({ minAge })` — is
 * what makes the query re-run on every change.
 */
export const peopleOver = defineQuery(
  "SELECT id, name, age, team FROM person WHERE age >= $minAge ORDER BY age",
);

export const peopleOf = defineQuery("SELECT id, name FROM person WHERE team = $team");

// ----------------------------------------------------------------- lives ---

export const livePeople = defineLive("SELECT id, name, age, team FROM person");

/**
 * Beat 3, nesting: one of these is mounted per team row, parameterised by the
 * outer row's `id`.
 *
 * Its text differs from `peopleOf` above on purpose. A cache entry is keyed by
 * text plus parameters and nothing else, so a live query and a one-shot query
 * spelled identically would share one entry — and whichever arrived first would
 * decide whether it was live.
 */
export const liveRoster = defineLive("SELECT id, name, age FROM person WHERE team = $team");

// --------------------------------------------------------------- writes ----

export const addPerson = defineQuery(
  "CREATE person SET name = $name, age = $age, team = $team",
);

export const removePerson = defineQuery("DELETE person WHERE id = $person");

// ----------------------------------------------------------------- teams ---

export const allTeams = defineQuery("SELECT id, name FROM team ORDER BY name");

// --------------------------------------------------------------- tickets ---

/**
 * Beat 4, record-level access control. There is nothing about identity in this
 * text — the `PERMISSIONS FOR select WHERE team = $auth.team` on the `ticket`
 * table is what makes the same query return different rows to different people.
 */
export const allTickets = defineQuery("SELECT id, title, done, team FROM ticket ORDER BY title");

// ===========================================================================
// THE EDITOR MOMENT
//
// Five one-line edits. Uncomment one, save, and SurrealGuard reports it — in
// the editor through the LSP, and on the command line with
// `surrealguard check`. Every message below is the real one, copied from a run
// against this schema; nothing here is paraphrased.
//
// Re-comment the line before moving on: `generate` refuses to write the
// registry while there is an error, so a stray one will make the app's types go
// stale on the next regeneration.
//
// --- a misspelled field ---
// error[E1002]: `person` has no field `nmae`
//   help: did you mean `name`?
//
// export const typo = defineQuery("SELECT id, nmae FROM person");
//
// --- a misspelled table ---
// error[E1001]: `prson` is not a defined table
//   help: did you mean `person`?
//
// export const noSuchTable = defineLive("SELECT id, name FROM prson");
//
// --- comparing an int field against a string ---
// error[E2004]: `>` can't combine a `int` and a `string`
//
// export const wrongType = defineQuery("SELECT id, name FROM person WHERE age > 'thirty'");
//
// --- a live query that tries to sort ---
// error[E4009]: a live query can't ORDER BY
//   help: a subscription delivers one change at a time, so there is no result
//         set to sort — order the rows on the client
//
// export const sorted = defineLive("SELECT id, name FROM person ORDER BY name");
//
// --- DIFF and FETCH together ---
// warning[W4027]: this FETCH does nothing — a DIFF notification is never fetched
//   help: notifications for this subscription carry a JSON-Patch array with the
//         link left as a record id; drop DIFF to get fetched rows, or resolve
//         the link on the client
//
// export const diffed = defineLive("LIVE SELECT DIFF FROM person FETCH team");
// ===========================================================================
