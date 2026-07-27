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

export const db = createClient({
  url: "ws://localhost:8000/rpc",
  namespace: "demo",
  database: "demo",
});
