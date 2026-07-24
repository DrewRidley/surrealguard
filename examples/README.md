# SurrealGuard examples

Real, type-checked demos of the `surrealguard generate` round-trip:

**schema (`.surql`) → `surrealguard generate` → typed `db.query` / `db.live`.**

`surrealguard generate` scans your host files (`.ts`, `.tsx`, `.svelte`, …) for
`db.query("…")` / `db.live(\`…\`)` calls, analyzes each query against your
`schema/*.surql`, and writes `surrealguard.generated.ts` — a module
augmentation that keys every query by its exact text with its `{ result; params }`
types. You import the client **from that generated file**, so the augmentation
loads with it and `db.query(...)` is fully typed: the result rows, and the
params argument (required exactly when the query reads params).

## Examples

| Example | What it shows |
| --- | --- |
| [`basic/`](./basic) | Vanilla TypeScript: `new SurrealGuardClient()` + typed `db.query(...)`. |
| [`sveltekit/`](./sveltekit) | Svelte 5 / SvelteKit: typed `liveQuery` runes + a `load` that seeds SSR rows. |

## Run the round-trip

From the repo root (the workspace links `@surrealguard/*` via `workspace:*`):

```sh
pnpm install

# 1. Build the CLI (any target dir works).
CARGO_TARGET_DIR=/tmp/sg cargo build --release -p surrealguard

# 2. Generate the typed registry from a schema + embedded queries.
cd examples/basic
/tmp/sg/release/surrealguard generate --out surrealguard.generated.ts

# 3. Type-check — the generated types make db.query fully typed.
cd ../..
pnpm --filter @surrealguard-example/basic exec tsc --noEmit
```

## The guarantee

The generated types are load-bearing, not decorative. Each example passes a
wrong param under a `// @ts-expect-error` line:

```ts
// @ts-expect-error team must be a RecordId<"team">, not a number.
await db.query("SELECT name FROM person WHERE team = $team", { team: 123 });
```

Delete that directive and `tsc --noEmit` fails with
`Type 'number' is not assignable to type 'RecordId<"team">'` — proving the
params really are checked against the schema.

## A note on the SvelteKit example

`tsc` type-checks the `.ts` data layer (`src/lib/*.ts`, `src/routes/+page.ts`),
which is where the typed `liveQuery` / `db.query` round-trip is proven. The
`.svelte` components are illustrative; to type-check those too, add
`svelte-check` and run it alongside `tsc`.
