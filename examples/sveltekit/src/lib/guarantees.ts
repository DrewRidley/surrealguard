// The typed round-trip, exercised so `tsc --noEmit` proves the registry types
// are real and load-bearing. Every `@ts-expect-error` below is a *compile*
// error — delete one and the typecheck fails.
//
// This file is never executed.

import { createLive, createMutation, createQuery, preload } from "@surrealguard/svelte";
import { RecordId, defineQuery } from "$lib/surrealguard.generated";
import { db } from "./db";
import { addPerson, allPeople, liveTeam, livePeople, peopleOf } from "./queries";

const team = new RecordId("team", "red");

/** A typed one-shot query: params required, result typed — both from the text. */
export async function personNames() {
  const rows = await db.run(peopleOf, { team });
  return rows.map((row) => row.name); // row.name is `string`
}

/** The SSR seed carries its result type across the boundary, as plain values. */
export async function seed() {
  const preloaded = await preload(db, livePeople);
  // A record link arrives as `person:${string}` — devalue-safe, and a usable
  // `{#each}` key.
  const ids: Array<`person:${string}`> = preloaded.data.map((person) => person.id);
  return ids;
}

export function reactive() {
  // A live query, and a one-shot query with loading/error state.
  const live = createLive(livePeople, { client: db });
  const once = createQuery(allPeople, { client: db });
  // A write, plus what it invalidates.
  const add = createMutation(addPerson, {
    client: db,
    invalidates: [allPeople, livePeople],
  });
  add.mutate({ name: "ada", age: 36, team });
  return { live, once, add };
}

export async function guarantees() {
  // ---- db.query, with nothing wrapped around it ---------------------------
  // The form a reader already knows is checked exactly as hard as the named
  // one. `src/routes/+page.svelte` proves the same thing inside a `.svelte`
  // file, which is where it actually has to hold.

  // @ts-expect-error `nope` is not in the generated result shape.
  (await db.query("SELECT id, name, age, team FROM person"))[0][0]!.nope;

  // @ts-expect-error a required param cannot be omitted.
  await db.query("SELECT id, name FROM person WHERE team = $team");

  // @ts-expect-error a plain string is not a RecordId<"team">.
  await db.query("SELECT id, name FROM person WHERE team = $team", { team: "team:red" });

  // ---- the same guarantees through a named query --------------------------

  // @ts-expect-error a required param cannot be omitted.
  await db.run(peopleOf);

  // @ts-expect-error a plain string is not a RecordId<"team">. This used to
  // typecheck and then match nothing on the wire.
  await db.run(peopleOf, { team: "team:red" });

  // @ts-expect-error a param-free query takes no params.
  await db.run(allPeople, { team });

  // @ts-expect-error `nope` is not in the generated result shape.
  (await db.run(allPeople))[0]!.nope;

  // @ts-expect-error a RecordId is not a string.
  (await db.run(allPeople))[0]!.team.startsWith("team:");

  // @ts-expect-error an UNBOUND live query cannot be handed to an adapter.
  createLive(liveTeam, { client: db });

  // @ts-expect-error a one-shot query is not a live query.
  createLive(allPeople, { client: db });

  // @ts-expect-error wrong mutation param type.
  createMutation(addPerson, { client: db }).mutate({ name: 1, age: 36, team });

  // A query text the registry does not contain is a hard error carrying its own
  // remedy, not a silent degrade to `unknown[]`. It cannot be demonstrated in a
  // generated example — `generate` extracts every `defineQuery` literal here, so
  // a miss is only ever a *stale* registry, and this example regenerates
  // cleanly. See `packages/client/test-d/query.test-d.ts`, where the registry is
  // hand-declared and no tool rewrites it.
  //
  // The deliberate opt-out is demonstrable, and degrades to `unknown[]` —
  // never `any`:
  const dynamic = defineQuery.unchecked(`SELECT * FROM ${"person"}`);
  const rows = await db.run(dynamic);
  // @ts-expect-error the row is `unknown`, so nothing can be read off it.
  rows[0]!.name;
}
