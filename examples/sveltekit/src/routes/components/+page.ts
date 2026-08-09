// The same server-side `preload` as `/live`, feeding `<Query>` instead of
// `createLive`. The payload remembers which query it is, so the component
// renders the server's rows on the first paint and never names the query.

import { preload } from "@surrealguard/svelte";
import { db } from "$lib/db";
import { allPeople } from "$lib/queries";

export async function load() {
  return { people: await preload(db, allPeople) };
}
