# @surrealguard/client

A typed SurrealQL client. `SurrealGuardClient` **extends** the official
[`surrealdb`](https://www.npmjs.com/package/surrealdb) SDK's `Surreal` class, so
every SDK method still works — but a string-literal query passed to `db.query`
resolves its result rows and its parameters from types generated off your
schema. No wrapper syntax, no hand-written result interfaces.

Nothing is typed until you run `surrealguard generate`, so start there.

## Install

```sh
npm i @surrealguard/client surrealdb
npm i -D surrealguard
```

`surrealdb` is a peer dependency. `surrealguard` is the CLI that generates the
types; it is a small launcher that downloads a prebuilt binary on first run.

## Generate the types

**1. Point the CLI at your schema.** `npx surrealguard init` writes a commented
`surrealguard.toml`; the part that matters is which `.surql` files are the
schema.

```toml
[sources]
schema = ["schema/**/*.surql"]
queries = ["queries/**/*.surql"]
ignore = ["node_modules/**"]
```

```surql
-- schema/schema.surql
DEFINE TABLE team SCHEMAFULL;
DEFINE FIELD name ON team TYPE string;

DEFINE TABLE person SCHEMAFULL;
DEFINE FIELD name ON person TYPE string;
DEFINE FIELD age ON person TYPE int;
DEFINE FIELD team ON person TYPE record<team>;
```

**2. Write the query as a string literal** in your own code. `generate` scans
`.ts`/`.tsx`/`.js`/`.jsx`/`.svelte`/`.vue`/`.astro` files for `db.query("…")`
and ``db.live(`…`)`` calls (and `` surql`…` `` templates) and analyzes each one
against the schema. The queries already in your source are the input; there is
no separate query manifest to maintain.

```ts
// src/main.ts
import { SurrealGuardClient } from "./surrealguard.generated";

const db = new SurrealGuardClient();
await db.connect("ws://localhost:8000/rpc");
await db.use({ namespace: "app", database: "app" });

const [people] = await db.query("SELECT name, age FROM person WHERE team = $team", {
  team: "team:red",
});

for (const person of people) {
  console.log(person.name, person.age);
}
```

**3. Run generate.**

```sh
npx surrealguard generate --out src/surrealguard.generated.ts
```

Without `--out` the file lands at `surrealguard.generated.ts` in the workspace
root (the directory holding `surrealguard.toml`). Put it wherever your imports
are convenient; these examples use `src/` so the import above is a plain
relative one.

The generated file re-exports the client **and** carries a
`declare module "@surrealguard/client"` augmentation:

```ts
declare module "@surrealguard/client" {
  interface SurqlRegistry {
    "SELECT name, age FROM person WHERE team = $team": {
      result: [Array<{ age: number; name: string }>];
      params: { team: RecordId<"team"> };
    };
  }
}
```

Import `SurrealGuardClient` **from the generated file**, not from
`@surrealguard/client`. That import is what loads the augmentation — otherwise
the registry is empty and every query degrades to `unknown[]`.

Re-run `generate` after changing a query or the schema. `generate` and `check`
support a watch mode (`--watch`) that stays running and regenerates on save;
`surrealguard generate --help` lists the flags your installed version has.

## Why the registry is keyed by the literal query text

TypeScript infers a literal type for a string *argument*, so
`db.query("SELECT …")` can key into an interface by that exact text. That is the
whole mechanism: one interface, one entry per query, keyed by its source. It also
explains two things that would otherwise look arbitrary.

- **Whitespace and casing must match exactly.** The key is the text, byte for
  byte. Reformatting a query changes its key, so re-run `generate`.
- **`db.live` takes the query as an argument, not a tagged template.**
  TypeScript widens a *tagged* template's cooked text to `string`
  ([#33304](https://github.com/microsoft/TypeScript/issues/33304)), which loses
  the literal and therefore the row type. ``db.live(`…`)`` (note the
  parentheses) and `db.live("…")` are the typed forms.

## The result is a per-statement tuple

SurrealDB returns one result per statement, in source order, so a query's
`result` is a tuple with one element per statement — a single-statement query is
a one-element tuple you destructure with `const [rows] =`.

```ts
const [names, ages] = await db.query("SELECT name FROM person; SELECT age FROM person");
//     ^ Array<{ name: string }>
//            ^ Array<{ age: number }>
```

A non-responding statement (`LET`, `DEFINE`, …) contributes `null`:

```ts
const [, people] = await db.query("LET $t = time::now(); SELECT name FROM person");
```

## Parameters

The params argument is required exactly when the query reads a `$param`, and
forbidden when it does not. Both are compile errors, not runtime surprises:

```ts
// @ts-expect-error the query reads $team, so params are required
await db.query("SELECT name, age FROM person WHERE team = $team");

// @ts-expect-error person.team is record<team>, so $team is a RecordId<"team">
await db.query("SELECT name, age FROM person WHERE team = $team", { team: 123 });
```

This is one conditional generic rather than an overload set on purpose: a
second, permissive `query(sql: string, params?: object)` overload would let a
registered query silently skip its required params, so there isn't one.

## Live queries

`db.live(...)` does not open a subscription — it returns a `LiveDescriptor<Row>`,
a plain object carrying the `LIVE SELECT …` text with the row type attached. The
reactive core ([`@surrealguard/query`](https://www.npmjs.com/package/@surrealguard/query))
and the framework adapters ([`@surrealguard/svelte`](https://www.npmjs.com/package/@surrealguard/svelte),
[`@surrealguard/next`](https://www.npmjs.com/package/@surrealguard/next)) consume
it.

```ts
const people = db.live(`SELECT name, age FROM person`);
people.sql; // "LIVE SELECT name, age FROM person" — the LIVE prefix is added for you
```

The SDK's own `db.live(table)` subscription still works: a non-string argument
passes straight through to it.

## Escape hatches

- **A query built at runtime** — a non-literal string is not in the registry and
  resolves to the SDK's `unknown[]` with optional bindings. It still runs. There
  is no `any` anywhere in this API.
- **A different output path** — `generate --out <path>`. The file re-exports a
  runtime value, so the extension must be `.ts`, not `.d.ts`.
- **The plain SDK** — `SurrealGuardClient` *is* a `Surreal`, so `db.select`,
  `db.create`, `db.merge`, transactions, and everything else are unchanged.

Part of [SurrealGuard](https://github.com/DrewRidley/surrealguard). Licensed
under MIT OR Apache-2.0.
