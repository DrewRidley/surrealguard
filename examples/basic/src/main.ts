// A vanilla-TypeScript SurrealGuard demo.
//
// `surrealguard generate` scanned this project, found every query text in it,
// analyzed each one against `schema/schema.surql`, and wrote
// `src/surrealguard.generated.ts` — a module augmentation that types every
// query by its exact text. We import the entry points from that generated file,
// so the augmentation loads with them and everything below is fully typed.

import { createClient, RecordId, SurrealGuardError } from "./surrealguard.generated";
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
  // ---- Start here: db.query ----------------------------------------------
  // Write the SurrealQL you already know. Nothing wraps it, nothing names it,
  // no helper is imported — and it is fully typed, because `generate` keyed the
  // registry by this exact text.
  //
  // SurrealDB returns one result per statement, so `query` hands back the
  // per-statement tuple. One statement, one element: destructure it.
  const [people] = await db.query("SELECT id, name, age, team FROM person");
  for (const person of people) {
    // `person.name` is `string` and `person.age` is `number`. `person.nope`
    // is a compile error — not `any`.
    console.log(person.name.toUpperCase(), person.age.toFixed(0));
  }

  // Params are checked from the same text, by the same lookup.
  const team = new RecordId("team", "red");
  const [red] = await db.query("SELECT id, name FROM person WHERE team = $team", {
    team,
  });
  console.log(red.map((person) => person.name));

  // ---- Record links are RecordId, and that is the whole point --------------
  // `person.team` is `record<team>` in the schema, so it decodes as a RecordId
  // — the SDK's own class, not a string. Reading it is honest:
  console.log(people[0]?.team.table, people[0]?.team.id);

  // …and WRITING it is why this matters. A RecordId parameter encodes to a
  // record link on the wire (CBOR tag 8); a plain string encodes to a SurrealQL
  // string, so `WHERE team = $team` matches only with the class. Passing
  // `"team:red"` above is a compile error rather than zero rows at runtime.

  // ---- Naming a query, when a name earns its keep -------------------------
  // `defineQuery` reads the same registry through the same conditional generic,
  // so it buys no extra type safety. What it buys is a VALUE: the text is
  // written once in `queries.ts` and imported everywhere, so two places cannot
  // drift apart. It is also what `db.run`, `db.watch` and `db.invalidate` take.
  //
  // `db.run` unwraps a single-statement query, so there is no destructure.
  const roster = await db.run(allPeople);
  console.log(roster.length);
  console.log((await db.run(peopleOf, { team })).map((person) => person.name));

  // A multi-statement query keeps the per-statement tuple. Nothing is hidden:
  // only a SINGLE-statement query unwraps.
  const [names, ages] = await db.run(namesAndAges);
  for (const { name } of names) console.log(name.toUpperCase());
  for (const { age } of ages) console.log(age.toFixed(0));

  // ---- Writing, and telling the cache about it ---------------------------
  await db.run(addPerson, { name: "ada", age: 36, team });
  await db.invalidate(allPeople); // every binding of that query
  await db.invalidate(peopleOf.with({ team })); // just that binding

  // ---- Live, with no other package ---------------------------------------
  // `@surrealguard/client` subscribes on its own. A live query is the case that
  // genuinely needs a named reference: `db.watch` holds on to it.
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
