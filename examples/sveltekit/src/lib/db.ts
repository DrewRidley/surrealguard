// The typed client. Importing `SurrealGuardClient` from the generated file also
// loads its `SurqlRegistry` augmentation, so every `db.query(...)` /
// `db.live(...)` below (and in any component) is typed from the query text.
import { SurrealGuardClient } from "../../surrealguard.generated";

export const db = new SurrealGuardClient();
