/**
 * Type-level tests for the **server** story — the App Router patterns the
 * README documents, compiled rather than assumed. There is no `examples/next`
 * app yet, so this is where the RSC surface is verified.
 *
 * Compiling this file IS the test. Nothing renders.
 */

import { Suspense } from "react";
import {
  defineLive,
  defineQuery,
  RecordId,
  type Json,
  type Preloaded,
} from "@surrealdb/analyzer-client";
import { createClient } from "@surrealdb/analyzer-client";
import { preload } from "../src/server.js";
import { useLive } from "../src/index.js";

declare module "@surrealdb/analyzer-client" {
  interface SurqlRegistry {
    "SELECT id, name, joined FROM person": {
      result: [Array<{ id: RecordId<"person">; name: string; joined: Date }>];
      params: Record<string, never>;
    };
    "SELECT count() FROM audit GROUP ALL": {
      result: [Array<{ count: number }>];
      params: Record<string, never>;
    };
    "SELECT id, name, age FROM person": {
      result: [Array<{ id: RecordId<"person">; name: string; age: number }>];
      params: Record<string, never>;
    };
    "SELECT id, name FROM person WHERE team = $team": {
      result: [Array<{ id: RecordId<"person">; name: string }>];
      params: { team: RecordId<"team"> };
    };
  }
}

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;
type Expect<T extends true> = T;

const allPeople = defineQuery("SELECT id, name, joined FROM person");
const slowReport = defineQuery("SELECT count() FROM audit GROUP ALL");
const livePeople = defineLive("SELECT id, name, joined FROM person");

/**
 * A per-request server client. React's `cache()` is what scopes it; a
 * module-level client on the server would share one connection, one auth
 * session and one query cache across every concurrent request and every user.
 *
 * Declared here rather than imported so this file does not require a React
 * version that exports `cache` from its public types.
 */
declare function cache<F extends (...args: never[]) => unknown>(fn: F): F;

/**
 * React 19's `use`. Declared rather than imported because this package
 * peer-depends on `react >= 18`, and `@types/react@18` does not have it — the
 * streaming pattern below needs React 19.
 */
declare function use<T>(promise: Promise<T>): T;

const getDb = cache(() =>
  createClient({
    url: "ws://localhost:8000/rpc",
    namespace: "app",
    database: "app",
  }),
);

// --- The README's headline: a Server Component and nothing else -------------
//
// No provider, no hook, no wrapper around the string. This is the first block
// of the README, so it is pinned here rather than left to prose.

export async function SimplePage() {
  const [people] = await getDb().query("SELECT id, name, age FROM person");
  type _direct = Expect<
    Equal<typeof people, Array<{ id: RecordId<"person">; name: string; age: number }>>
  >;
  // @ts-expect-error `nope` is not on the row — a miss is an error, not `any`.
  void people[0]?.nope;

  // Params are required exactly when the text reads them, and typed.
  const [red] = await getDb().query("SELECT id, name FROM person WHERE team = $team", {
    team: new RecordId("team", "red"),
  });
  // @ts-expect-error the text reads $team, so the params object is required.
  await getDb().query("SELECT id, name FROM person WHERE team = $team");

  return (
    <ul>
      {people.map((p) => (
        <li key={String(p.id)}>
          {p.name} {red.length}
        </li>
      ))}
    </ul>
  );
}

// --- A Server Component reading data directly (zero client JS) --------------

export async function PeoplePage() {
  // `runJson`, not `run`: an RSC -> client-component boundary accepts only
  // plain values and has no transport hook, so a RecordId instance throws
  // "Only plain objects can be passed to Client Components".
  const people = await getDb().runJson(allPeople);
  type _json = Expect<
    Equal<typeof people, Array<{ id: `person:${string}`; name: string; joined: string }>>
  >;

  // Server-only use keeps the SDK's real values.
  const sdkValues = await getDb().run(allPeople);
  type _sdk = Expect<
    Equal<typeof sdkValues, Array<{ id: RecordId<"person">; name: string; joined: Date }>>
  >;
  sdkValues[0]?.joined.getTime();

  const preloaded = await preload(getDb(), livePeople);
  type _preloaded = Expect<
    Equal<
      typeof preloaded,
      Preloaded<Array<{ id: `person:${string}`; name: string; joined: string }>>
    >
  >;

  return (
    <>
      <ul>
        {people.map((person) => (
          <li key={person.id}>{person.name}</li>
        ))}
      </ul>
      <PeopleList preloaded={preloaded} />
    </>
  );
}

// --- The client component it seeds ------------------------------------------

type Person = Json<{ id: RecordId<"person">; name: string; joined: Date }>;

function PeopleList({ preloaded }: { preloaded: Preloaded<Person[]> }) {
  // Hydrates from the server's rows, then upgrades to live — with no second
  // reference to the query text anywhere in this file.
  const people = useLive(preloaded);
  type _rows = Expect<Equal<typeof people.data, Person[]>>;
  return (
    <ul>
      {people.data.map((person) => (
        <li key={person.id}>{person.name}</li>
      ))}
    </ul>
  );
}

// --- Streaming a slow query: pass an UN-awaited promise, `use()` it ---------

export function ReportPage() {
  const rows = getDb().runJson(slowReport); // deliberately not awaited
  return (
    <Suspense fallback={<p>loading…</p>}>
      <Report rows={rows} />
    </Suspense>
  );
}

function Report({ rows }: { rows: Promise<Array<{ count: number }>> }) {
  const data = use(rows);
  type _streamed = Expect<Equal<typeof data, Array<{ count: number }>>>;
  return <p>{data[0]?.count}</p>;
}
