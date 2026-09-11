/**
 * Turning a reactive row's record link back into a parameter.
 *
 * This is the one rough edge in the nested-live-query pattern, and it is not
 * obvious from either side, so it lives here rather than being re-typed in
 * every app.
 *
 * Everything the reactive layer hands you is {@link Json}-shaped — that is what
 * makes it survive `load`, devalue and hydration — so a `record<team>` field
 * arrives as the string `` `team:${string}` ``. Query PARAMETERS are the SDK's
 * values, because a `RecordId` encodes to a record link on the wire while a
 * plain string encodes to a SurrealQL string, and `WHERE team = $team` only
 * matches with the former. Feeding an outer row's link into an inner query
 * therefore needs the link reconstructed:
 *
 * ```svelte
 * <Query q={allPeople}>
 *   {#snippet children(people)}
 *     {#each people as person (person.id)}
 *       <!-- person.team is "team:red"; the parameter must be a RecordId -->
 *       <LiveQuery q={liveTeam.with({ team: recordId(person.team) })}>
 *         {#snippet children(teammates)}<small>{teammates.length}</small>{/snippet}
 *       </LiveQuery>
 *     {/each}
 *   {/snippet}
 * </Query>
 * ```
 *
 * The table name survives in the literal type, which is what lets this infer
 * `RecordId<"team">` from `` `team:${string}` `` with no annotation and no cast
 * at the call site.
 */

import { RecordId } from "surrealdb";

/**
 * Rebuild the `RecordId` behind a JSON record link: `"team:red"` →
 * `RecordId<"team">`.
 *
 * The table is everything before the first `:`, which is where SurrealDB puts
 * it; a record id containing a `:` of its own (`team:⟨a:b⟩`) keeps it, since
 * only the first separator is consumed.
 */
export function recordId<Table extends string>(link: `${Table}:${string}`): RecordId<Table> {
  const separator = link.indexOf(":");
  if (separator < 0) {
    throw new TypeError(
      `[@surrealdb/analyzer-client] recordId: ${JSON.stringify(link)} is not a record link — ` +
        "expected `table:id`.",
    );
  }
  return new RecordId(link.slice(0, separator) as Table, link.slice(separator + 1));
}
