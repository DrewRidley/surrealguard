// The guarantee, made concrete. Every line below is a *compile* error, asserted
// with `@ts-expect-error` — delete one and `tsc --noEmit` fails, which is the
// whole point of generating types.
//
// This file is never executed.

import { createClient, defineQuery, RecordId } from "../surrealguard.generated";
import { allPeople, liveTeam, peopleOf } from "./queries";

const db = createClient({ url: "ws://localhost:8000/rpc" });
const team = new RecordId("team", "red");

export async function guarantees() {
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

  // A query text the registry does not contain is a hard error, not a silent
  // degrade to `unknown[]`. The compiler prints the remedy:
  //   Argument of type 'SurqlError<"this query is not in the generated
  //   registry - run `surrealguard generate`">' is not assignable to …
  const stale = defineQuery("SELECT nope FROM nowhere");
  // @ts-expect-error the generated file does not know this query.
  await db.run(stale);
}
