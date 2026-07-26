/**
 * Type-level tests for the React hooks. Compiling this file IS the test:
 * positive lines must typecheck and every `@ts-expect-error` line must fail.
 *
 * Pure type checks; nothing here renders.
 */

import {
  defineLive,
  defineQuery,
  preload,
  RecordId,
  type Preloaded,
  type SurrealGuardClient,
} from "@surrealguard/client";
import { useLive, useMutation, useQuery } from "../src/index.js";

declare module "@surrealguard/client" {
  interface SurqlRegistry {
    "SELECT id, name, age FROM person": {
      result: [Array<{ id: RecordId<"person">; name: string; age: number }>];
      params: Record<string, never>;
    };
    "SELECT id, name FROM person WHERE team = $team": {
      result: [Array<{ id: RecordId<"person">; name: string }>];
      params: { team: RecordId<"team"> };
    };
    "CREATE person SET name = $name, joined = $joined": {
      result: [Array<{ id: RecordId<"person">; name: string }>];
      params: { name: string; joined: Date };
    };
    "RETURN count(SELECT id FROM person)": {
      result: [number];
      params: Record<string, never>;
    };
  }
}

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;
type Expect<T extends true> = T;

const allPeople = defineQuery("SELECT id, name, age FROM person");
const peopleOf = defineQuery("SELECT id, name FROM person WHERE team = $team");
const peopleCount = defineQuery("RETURN count(SELECT id FROM person)");
const addPerson = defineQuery("CREATE person SET name = $name, joined = $joined");
const livePeople = defineLive("SELECT id, name, age FROM person");
const liveTeam = defineLive("SELECT id, name FROM person WHERE team = $team");

declare const db: SurrealGuardClient;
declare const team: RecordId<"team">;
declare const enabled: boolean;

async function component() {
  // 1. A live query's rows are the JSON projection — what actually survives a
  //    React Server Component boundary — and `data` is always an array.
  const people = useLive(livePeople);
  type _rows = Expect<
    Equal<typeof people.data, Array<{ id: `person:${string}`; name: string; age: number }>>
  >;
  people.data.map((person) => person.name.toUpperCase());

  // 2. The reference is passed DIRECTLY — no thunk. Its `key` is stable, so the
  //    hook's memo dependency is `[client, source.key]`.
  const forTeam = useLive(liveTeam.with({ team }));
  type _bound = Expect<Equal<typeof forTeam.data, Array<{ id: `person:${string}`; name: string }>>>;

  // 3. `"skip"` opts out and keeps the row type.
  const maybe = useLive(enabled ? liveTeam.with({ team }) : "skip");
  type _skip = Expect<Equal<typeof maybe.data, Array<{ id: `person:${string}`; name: string }>>>;

  // 4. A one-shot query's `data` is optional, because it may be a scalar.
  const roster = useQuery(allPeople);
  type _oneShot = Expect<
    Equal<
      typeof roster.data,
      Array<{ id: `person:${string}`; name: string; age: number }> | undefined
    >
  >;
  const count = useQuery(peopleCount);
  type _scalar = Expect<Equal<typeof count.data, number | undefined>>;

  // 5. `Preloaded<T>` carries the result type from the RSC into the client
  //    component, which recovers it with NO second reference to the query text.
  const preloaded = await preload(db, livePeople);
  type _preloaded = Expect<
    Equal<
      typeof preloaded,
      Preloaded<Array<{ id: `person:${string}`; name: string; age: number }>>
    >
  >;
  const hydrated = useLive(preloaded);
  type _hydrated = Expect<
    Equal<typeof hydrated.data, Array<{ id: `person:${string}`; name: string; age: number }>>
  >;

  // 6. Mutation params are checked.
  const add = useMutation(addPerson, { invalidates: [allPeople, livePeople] });
  add.mutate({ name: "ada", joined: new Date() });
  const created = await add.mutateAsync({ name: "ada", joined: new Date() });
  type _created = Expect<Equal<typeof created, Array<{ id: RecordId<"person">; name: string }>>>;

  // --- negatives: each must fail to compile --------------------------------

  // @ts-expect-error an UNBOUND live query cannot be handed to a hook
  useLive(liveTeam);
  // @ts-expect-error a one-shot query is not a live query
  useLive(allPeople);
  // @ts-expect-error an unbound query cannot be handed to useQuery
  useQuery(peopleOf);
  // @ts-expect-error an unbound live query cannot be preloaded
  await preload(db, liveTeam);
  // @ts-expect-error missing mutation params
  add.mutate();
  // @ts-expect-error wrong mutation param type
  add.mutate({ name: 1, joined: new Date() });
  // @ts-expect-error unknown row field
  people.data[0]!.nope;
  // @ts-expect-error a live row's id is a JSON string, not a RecordId
  people.data[0]!.id.table;

  void [forTeam, maybe, roster, count, hydrated];
}
void component;
