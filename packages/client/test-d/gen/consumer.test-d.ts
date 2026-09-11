// Proves the generated file feeds the whole client end to end, through a single
// import. Importing from the generated module also loads its
// `declare module "@surrealdb/analyzer-client"` augmentation, so the typed registry is
// live without a separate side-import — and the SDK value classes come with it,
// so a `RecordId` parameter can be constructed without a second package.
//
// Pure type checks — `main` is never called, so no connection opens.
import {
  createClient,
  defineQuery,
  RecordId,
  type Json,
} from "./surrealql-analyzer.generated.js";
import type { Equal, Expect } from "../assert.js";

const db = createClient({ url: "ws://localhost:8000/rpc" });
const peopleOf = defineQuery("SELECT name FROM person WHERE team = $team");
const roster = defineQuery("SELECT id, name, joined FROM person");

async function main() {
  // THE HEADLINE CLAIM, asserted exactly: a plain `db.query("…")` — no
  // `defineQuery`, no ceremony — resolves the real per-statement tuple through
  // nothing but the generated file's augmentation. This is the first thing
  // every README shows, so it is pinned here rather than left to prose.
  const direct = await db.query("SELECT id, name, joined FROM person");
  type _direct = Expect<
    Equal<typeof direct, [Array<{ id: RecordId<"person">; joined: Date; name: string }>]>
  >;
  direct[0][0]!.name.length;
  direct[0][0]!.joined.getTime();
  // @ts-expect-error `nope` is not on the row — a miss is an error, not `any`.
  direct[0][0]!.nope;

  // Params too: required exactly when the text reads them, and typed.
  const [directRed] = await db.query("SELECT name FROM person WHERE team = $team", {
    team: new RecordId("team", "red"),
  });
  directRed[0]!.name.length;
  // @ts-expect-error the text reads $team, so the params object is required.
  await db.query("SELECT name FROM person WHERE team = $team");
  // @ts-expect-error a string is not a RecordId<"team">.
  await db.query("SELECT name FROM person WHERE team = $team", { team: "team:red" });

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
