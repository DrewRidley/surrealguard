// The typed client. Importing `createClient` from the generated file also loads
// its `SurqlRegistry` augmentation, so every query below (and in any component)
// is typed from its text.
//
// The connection opens lazily on first use, so this module-level client is
// safe and no route has to remember to connect it. (0.4's example exported
// `new SurrealGuardClient()` and *nothing ever called `connect`*.)
import { createClient } from "../../surrealguard.generated";

export const db = createClient({
  url: "ws://localhost:8000/rpc",
  namespace: "demo",
  database: "demo",
});
