/**
 * Client context. Set the client once at the root of the app — in
 * `+layout.svelte` — and every {@link liveQuery} below it resolves that client
 * from context, so components never thread `db` through props.
 *
 * ```svelte
 * <!-- +layout.svelte -->
 * <script>
 *   import { setClient } from "@surrealguard/svelte";
 *   import { SurrealGuardClient } from "$lib/surrealguard.generated";
 *   setClient(new SurrealGuardClient());
 * </script>
 * <slot />
 * ```
 */

import { getContext, setContext } from "svelte";
import type { SurrealGuardClient } from "@surrealguard/client";

const CLIENT_KEY = Symbol.for("@surrealguard/svelte:client");

/** Provide the client to descendant components. Call in the root `+layout.svelte`. */
export function setClient(client: SurrealGuardClient): SurrealGuardClient {
  setContext(CLIENT_KEY, client);
  return client;
}

/**
 * Read the client from context. Throws with a clear message if no
 * {@link setClient} ran in an ancestor and no explicit `client` was passed —
 * failing fast beats a confusing "cannot read property of undefined".
 */
export function getClient(): SurrealGuardClient {
  const client = getContext<SurrealGuardClient | undefined>(CLIENT_KEY);
  if (!client) {
    throw new Error(
      "[@surrealguard/svelte] No client in context. Call setClient(db) in your root " +
        "+layout.svelte, or pass { client } to liveQuery.",
    );
  }
  return client;
}
