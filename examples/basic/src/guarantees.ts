// The guarantee, made concrete. Every line below is a *compile* error, asserted
// with `@ts-expect-error` — delete one and `tsc --noEmit` fails, which is the
// whole point of generating types.
//
// This file is never executed.

import { createClient, defineQuery, RecordId } from "./surrealql-analyzer.generated";
import { allPeople, liveTeam, peopleOf } from "./queries";

const db = createClient({ url: "ws://localhost:8000/rpc" });
const team = new RecordId("team", "red");

export async function guarantees() {
  // ---- db.query, with nothing wrapped around it ---------------------------
  // The plain literal form is checked exactly as hard as the named one.

  // @ts-expect-error `nope` is not in the generated result shape.
  (await db.query("SELECT id, name, age, team FROM person"))[0][0]!.nope;

  // @ts-expect-error a required param cannot be omitted.
  await db.query("SELECT id, name FROM person WHERE team = $team");

  // @ts-expect-error a plain string is not a RecordId<"team">.
  await db.query("SELECT id, name FROM person WHERE team = $team", { team: "team:red" });

  // ---- the same guarantees through a named query --------------------------

  // @ts-expect-error a required param cannot be omitted.
  await db.run(peopleOf);

  // @ts-expect-error a plain string is not a RecordId<"team">. This is the bug
  // that used to typecheck and then match nothing on the wire.
  await db.run(peopleOf, { team: "team:red" });

  // @ts-expect-error a param-free query takes no params.
  await db.run(allPeople, { team });

  // @ts-expect-error `nope` is not in the generated result shape.
  (await db.run(allPeople))[0]!.nope;

  // @ts-expect-error a RecordId is not a string — no string methods on a link.
  (await db.run(allPeople))[0]!.team.startsWith("team:");

  // @ts-expect-error the wrong param type is caught at bind time too.
  peopleOf.with({ team: 123 });

  // @ts-expect-error an unbound live query cannot be watched.
  db.watch(liveTeam, () => {});

  // A query text the registry does not contain is a hard error carrying its own
  // remedy, not a silent degrade to `unknown[]`:
  //   Argument of type 'SurqlError<"this query is not in the generated
  //   registry - run `surrealql-analyzer generate`">' is not assignable to …
  //
  // That guarantee CANNOT be demonstrated here, and the reason is worth stating.
  // `surrealql-analyzer generate` extracts every `defineQuery` / `defineLive` literal
  // in this project, so any query written in this file is, by construction, in
  // the generated file. A registry miss is therefore only ever a *stale*
  // registry — and an example that regenerates cleanly is exactly one that
  // cannot hold a stale one. (Reflowing a literal is the same thing: it changes
  // the key, so it errors until the next `generate`, which then picks the new
  // text up.) The guarantee is proven where the registry is hand-declared and
  // no tool rewrites it: `packages/client/test-d/query.test-d.ts`.
  //
  // What IS demonstrable is the deliberate opt-out, for a genuinely dynamic
  // query that no generated file could ever contain:
  const dynamic = defineQuery.unchecked(`SELECT * FROM ${"person"}`);
  const rows = await db.run(dynamic);
  // It degrades to `unknown[]` — never `any`, so the rows stay unusable until
  // you narrow them yourself, and nothing silently typechecks.
  // @ts-expect-error the row is `unknown`, so nothing can be read off it.
  rows[0]!.name;
}
