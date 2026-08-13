// Create the client once; import it anywhere.
//
// Importing `createClient` from the GENERATED file is what loads the
// `SurqlRegistry` augmentation, so every `db.query("…")` in the app — in a
// component, a `load`, a server route — is typed from its text. Importing it
// from `@surrealguard/client` instead would compile and give you `unknown[]`.
//
// The connection opens lazily on first use, so this module-level client is
// safe and no route has to remember to connect it.
//
// Nothing has to be "provided" to use this. `db.query` / `db.run` / `db.watch`
// work from this plain import. Only the reactive helpers in
// `@surrealguard/svelte` look for a client in context — see `+layout.svelte`.
import { createClient } from "$lib/surrealguard.generated";

/** Port 8124, not 8000 — see `scripts/db.mjs`. The two must agree. */
export const RPC_URL = "ws://127.0.0.1:8124/rpc";

/**
 * The same server's HTTP health endpoint, polled by `$lib/health.svelte.ts`.
 *
 * It exists because `Surreal.connect()` does NOT reject when there is nothing
 * listening — it waits, indefinitely — so a page opened before the database is
 * up shows four spinners and no explanation. That is a fine default for a
 * flaky network and a terrible one for a demo, and no error snippet can rescue
 * it, because no error is ever produced. So the app asks separately.
 */
export const HEALTH_URL = "http://127.0.0.1:8124/health";

export const db = createClient({
  url: RPC_URL,
  namespace: "demo",
  database: "demo",
  // `authentication`, not `signin`: the SDK reuses these credentials whenever a
  // session expires or the socket reconnects, so the demo survives a laptop
  // sleeping. A `.signin()` call — which the access-control beat makes — takes
  // precedence over this for the rest of that session.
  //
  // Root, because a root user BYPASSES table permissions: that is what makes
  // "signed out" mean "sees every ticket" in the last beat. A real app would
  // never ship this.
  authentication: { username: "root", password: "root" },
});
