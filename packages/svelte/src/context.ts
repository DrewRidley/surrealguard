/**
 * Client context. Set the client once at the root of the app — in
 * `+layout.svelte` — and every primitive below it resolves that client from
 * context, so components never thread `db` through props.
 *
 * ```svelte
 * <!-- src/routes/+layout.svelte -->
 * <script lang="ts">
 *   import { setClient } from "@surrealguard/svelte";
 *   import { db } from "$lib/db";
 *
 *   setClient(db);
 *   let { children } = $props();
 * </script>
 *
 * {@render children()}
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
 * Read the client from context. An explicit `override` wins (tests, a second
 * connection). Throws with a clear message if neither is present — failing fast
 * beats a confusing "cannot read property of undefined".
 *
 * `use*` for context and imperative access, `create*` for reactive primitives,
 * is TanStack Svelte v6's split; matching it exactly is free consistency for
 * anyone arriving from there.
 */
export function useClient(override?: SurrealGuardClient): SurrealGuardClient {
  const client = override ?? getContext<SurrealGuardClient | undefined>(CLIENT_KEY);
  if (!client) {
    throw new Error(
      "[@surrealguard/svelte] No client in context. Call setClient(db) in your root " +
        "+layout.svelte, or pass { client } to the primitive.",
    );
  }
  return client;
}
