// A shared reactive query, living in a `.svelte.ts` module — the idiomatic
// Svelte 5 way to share reactive state.
//
// This could not be done in 0.4: `liveQuery` called `$effect`, which only runs
// inside a component, so sharing a query from a module threw `effect_orphan`
// unless the caller wrapped it in `$effect.root` and managed disposal by hand.
// The primitives now use `createSubscriber`, which subscribes lazily on first
// read inside a tracking scope and tears down automatically, anywhere.
//
// `setContext` is only readable during component initialisation, so a module
// passes the client explicitly.

import { createLive, createQuery } from "@surrealguard/svelte";
import type { RecordId } from "@surrealguard/client";
import { db } from "./db";
import { allPeople, liveRoster, livePeople } from "./queries";

/** Every person, live. Import this from any component; they share one `LIVE SELECT`. */
export const people = createLive(livePeople, { client: db });

/** The same roster, fetched once, with loading and error state. */
export const roster = createQuery(allPeople, { client: db });

/**
 * A live query whose parameter is reactive. The THUNK is the point: it re-runs
 * whenever `team` changes, so the subscription follows the state. 0.4 read
 * `{ params }` once at construction, so navigating to another team did nothing.
 */
export function peopleOfTeam(team: () => RecordId<"team"> | undefined) {
  return createLive(
    () => {
      const current = team();
      // `"skip"` is how a thunk-based API says "not yet".
      return current ? liveRoster.with({ team: current }) : "skip";
    },
    { client: db },
  );
}
