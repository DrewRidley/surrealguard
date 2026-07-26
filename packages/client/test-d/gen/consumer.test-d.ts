// Proves the generated file feeds the whole client end to end, through a single
// import. Importing from the generated module also loads its
// `declare module "@surrealguard/client"` augmentation, so the typed registry is
// live without a separate side-import — and the SDK value classes come with it,
// so a `RecordId` parameter can be constructed without a second package.
//
// Pure type checks — `main` is never called, so no connection opens.
import {
  createClient,
  defineQuery,
  RecordId,
  type Json,
} from "./surrealguard.generated.js";
import type { Equal, Expect } from "../assert.js";

const db = createClient({ url: "ws://localhost:8000/rpc" });
const peopleOf = defineQuery("SELECT name FROM person WHERE team = $team");
const roster = defineQuery("SELECT id, name, joined FROM person");

async function main() {
  // Resolved from the generated registry entry — params required and typed as
  // the SDK class, which is what makes the query match on the wire.
  const rows = await db.run(peopleOf, { team: new RecordId("team", "red") });
  rows[0]!.name.length;

  // Values are the SDK's, so a datetime really is a Date and a record link
  // really is a RecordId.
  const people = await db.run(roster);
  type _row = Expect<
    Equal<(typeof people)[number], { id: RecordId<"person">; joined: Date; name: string }>
  >;
  people[0]!.joined.getTime();
  people[0]!.id.table;

  // …and `Json<T>` is what survives an SSR boundary.
  const asJson = await db.runJson(roster);
  type _json = Expect<
    Equal<(typeof asJson)[number], { id: `person:${string}`; joined: string; name: string }>
  >;
  asJson[0]!.id.startsWith("person:");

  // @ts-expect-error the generated entry requires the params object
  await db.run(peopleOf);
  // @ts-expect-error a string is not a RecordId — the encode-time bug, caught
  await db.run(peopleOf, { team: "team:red" });
  // @ts-expect-error field not in the generated result shape
  (await db.run(peopleOf, { team: new RecordId("team", "red") }))[0]!.age;
  // @ts-expect-error a RecordId is not a string: no string methods on a record link
  people[0]!.id.startsWith("person:");
}
void main;
