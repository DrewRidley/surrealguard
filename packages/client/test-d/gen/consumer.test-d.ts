// Proves the generated augmentation feeds SurrealGuardClient.query end to end.
// Imports by package name (as a consumer would) so the generated
// `declare module "@surrealguard/client"` augmentation applies to the same
// module identity the client resolves from.
import { SurrealGuardClient, type Connection } from "@surrealguard/client";
import "./surrealguard.generated.js";

declare const conn: Connection;
const db = new SurrealGuardClient(conn);

async function main() {
  // Resolved from the generated registry entry — params required + typed.
  const team = await db.query("SELECT name FROM person WHERE team = $team", { team: "red" });
  team[0]!.name.length;

  // @ts-expect-error generated entry requires the params object
  await db.query("SELECT name FROM person WHERE team = $team");
  // @ts-expect-error field not in the generated result shape
  (await db.query("SELECT name FROM person WHERE team = $team", { team: "red" }))[0]!.age;
}
void main;
