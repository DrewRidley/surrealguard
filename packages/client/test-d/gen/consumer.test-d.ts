// Proves the generated file feeds SurrealGuardClient.query end to end, with a
// single import. Importing `SurrealGuardClient` from the generated module also
// loads its `declare module "@surrealguard/client"` augmentation, so the typed
// registry is live without a separate side-import.
//
// Pure type checks — `main` is never called, so no connection opens.
import { SurrealGuardClient } from "./surrealguard.generated.js";

const db = new SurrealGuardClient();

async function main() {
  // Resolved from the generated registry entry — params required + typed.
  const [team] = await db.query("SELECT name FROM person WHERE team = $team", { team: "red" });
  team[0]!.name.length;

  // @ts-expect-error generated entry requires the params object
  await db.query("SELECT name FROM person WHERE team = $team");
  // @ts-expect-error field not in the generated result shape
  (await db.query("SELECT name FROM person WHERE team = $team", { team: "red" }))[0]!.age;
}
void main;
