// A vanilla-TypeScript SurrealGuard demo.
//
// `surrealguard generate` scanned this project, found the query text in
// `src/queries.ts`, analyzed each query against `schema/schema.surql`, and
// wrote `surrealguard.generated.ts` — a module augmentation that types every
// query by its exact text. We import the entry points from that generated file,
// so the augmentation loads with them and everything below is fully typed.

import { createClient, RecordId, SurrealGuardError } from "../surrealguard.generated";
import {
  addPerson,
  allPeople,
  liveTeam,
  livePeople,
  namesAndAges,
  peopleOf,
} from "./queries";

// The connection opens lazily on first use, so a module-level client is safe
// and nothing has to remember to `await db.connect(...)`.
const db = createClient({
  url: "ws://localhost:8000/rpc",
  namespace: "demo",
  database: "demo",
});

async function main() {
  // ---- Reading ------------------------------------------------------------
  // A single-statement query resolves to its rows directly — no destructure.
  // Result and params are both inferred from the query text against the schema.
  const people = await db.run(allPeople);
  for (const person of people) {
    // `person.name` is a string; `person.nope` would be a compile error.
    console.log(person.name.toUpperCase(), person.age.toFixed(0));
  }

  // ---- Record links are RecordId, and that is the whole point --------------
  // `person.team` is `record<team>` in the schema, so it decodes as a RecordId
  // — the SDK's own class, not a string. Reading it is honest:
  console.log(people[0]?.team.table, people[0]?.team.id);

  // …and WRITING it is why this matters. A RecordId parameter encodes to a
  // record link on the wire (CBOR tag 8); a plain string encodes to a SurrealQL
  // string. `WHERE team = $team` matches only with the class, so the 0.4 shape
  // — a branded string — returned zero rows, silently.
  const team = new RecordId("team", "red");
  const red = await db.run(peopleOf, { team });
  console.log(red.map((person) => person.name));

  // ---- Multi-statement: the per-statement tuple stays ---------------------
  // SurrealDB returns one result per statement, so nothing is hidden: only a
  // SINGLE-statement query unwraps.
  const [names, ages] = await db.run(namesAndAges);
  for (const { name } of names) console.log(name.toUpperCase());
  for (const { age } of ages) console.log(age.toFixed(0));

  // ---- Writing, and telling the cache about it ---------------------------
  await db.run(addPerson, { name: "ada", age: 36, team });
  await db.invalidate(allPeople);           // every binding of that query
  await db.invalidate(peopleOf.with({ team })); // just that binding

  // ---- Live, with no other package ---------------------------------------
  // `@surrealguard/client` can subscribe on its own. In 0.4, `db.live(...)`
  // returned an inert descriptor and receiving a row required installing
  // `@surrealguard/query` and finding `getQueryClient(db).observeLive(...)`.
  const stop = db.watch(livePeople, (rows) => {
    for (const row of rows) console.log("live:", row.name, row.age);
  });

  // A live query with parameters must be bound before it can be watched — one
  // uniform rule, enforced by the compiler.
  const stopTeam = db.watch(liveTeam.with({ team }), (rows) => {
    console.log("red team:", rows.length);
  });

  // ---- Crossing a serialisation boundary ---------------------------------
  // `Json<T>` is the SDK's own projection: a RecordId becomes `person:${string}`
  // and a datetime becomes a string, so this survives JSON.stringify, devalue,
  // and a React Server Component's props.
  const serialisable = await db.runJson(allPeople);
  console.log(JSON.stringify(serialisable));
  console.log(serialisable[0]?.team.startsWith("team:"));

  stop();
  stopTeam();
  await db.close();
}

// ---- Errors carry the query that failed ----------------------------------
void main().catch((error: unknown) => {
  if (error instanceof SurrealGuardError) {
    console.error("query failed:", error.query, error.params, error.cause);
  } else {
    throw error;
  }
});
