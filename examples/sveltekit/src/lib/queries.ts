// The two writes the demo's buttons make.
//
// Reads are written inline, in the markup, where you are looking when you want
// to change them:
//
//   <LiveQuery q="SELECT id, name, age, team FROM person WHERE age > {minAge}">
//
// A write has no component to hang off, so it is named here and handed to
// `createMutation`. That is what `defineQuery` is for — giving a query an
// identity a value can carry — and it adds no type safety of its own: the
// parameters and result below are read out of the same generated registry the
// inline attributes are.

import { defineQuery } from "$lib/surrealguard.generated";

export const addPerson = defineQuery(
  "CREATE person SET name = $name, age = $age, team = $team",
);

export const removePerson = defineQuery("DELETE person WHERE id = $person");

// ===========================================================================
// THE EDITOR MOMENT
//
// Three one-line edits. Uncomment one, save, and SurrealGuard reports it — in
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
// (add `defineLive` to the import above for that one)
// ===========================================================================
