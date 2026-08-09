// Turning a reactive row's record link back into a parameter.
//
// This is worth a file of its own because it is the one rough edge in the
// nested-live-query pattern, and it is not obvious from either side.
//
// Everything the reactive layer hands you is `Json`-shaped — that is what makes
// it survive `load`, devalue and hydration — so a `record<team>` field arrives
// as the string `` `team:${string}` ``. Query PARAMETERS are the SDK's values,
// because a `RecordId` encodes to a record link on the wire while a plain
// string encodes to a SurrealQL string, and `WHERE team = $team` only matches
// with the former. So feeding an outer row's link into an inner query needs the
// link reconstructed.
//
// The table name survives in the literal type, which is what lets this infer
// `RecordId<"team">` from `` `team:${string}` `` with no annotation and no cast
// at the call site.

import { RecordId } from "$lib/surrealguard.generated";

/** Rebuild the `RecordId` behind a JSON record link: `"team:red"` → `RecordId<"team">`. */
export function recordId<Table extends string>(link: `${Table}:${string}`): RecordId<Table> {
  const separator = link.indexOf(":");
  return new RecordId(link.slice(0, separator) as Table, link.slice(separator + 1));
}
