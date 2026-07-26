// SvelteKit `load`: fetch on the server so the first paint is gap-free, then
// the component upgrades to live on the client.
//
// `preload` returns a payload that REMEMBERS which query it is — its key, text
// and params travel with the rows — so `+page.svelte` subscribes to exactly
// this query without naming it again. In 0.4 the component had to repeat the
// query text byte-for-byte or silently lose the seed.
//
// The payload is plain values (a RecordId is already `person:${string}`), so
// devalue accepts it without a `transport` hook.

import { preload } from "@surrealguard/svelte";
import { db } from "$lib/db";
import { livePeople } from "$lib/queries";

export async function load() {
  const people = await preload(db, livePeople);

  // Fully typed from the schema, with no cast: reading a bogus field here is a
  // compile error.
  const names: string[] = people.data.map((person) => person.name);

  return { people, names };
}
