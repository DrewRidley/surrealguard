// THE EDITOR MOMENT
//
// Uncomment a line, save, and SurrealGuard reports it — in the editor through
// the LSP, and on the command line with `surrealguard check`. Every message
// below is the real one, copied from a run against this schema.
//
// Re-comment it before moving on: `generate` refuses to write the registry
// while there is an error, so a stray one leaves the app's types stale.
//
// import { defineLive, defineQuery } from "$lib/surrealguard.generated";
//
// error[E1002]: `person` has no field `nmae`   help: did you mean `name`?
// export const typo = defineQuery("SELECT id, nmae FROM person");
//
// error[E2004]: `>` can't combine a `int` and a `string`
// export const wrongType = defineQuery("SELECT id FROM person WHERE age > 'thirty'");
//
// error[E4009]: a live query can't ORDER BY
//   help: a subscription delivers one change at a time, so there is no result
//         set to sort — order the rows on the client
// export const sorted = defineLive("SELECT id, name FROM person ORDER BY name");

export {};
