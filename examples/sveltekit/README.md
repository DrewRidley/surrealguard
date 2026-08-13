# SurrealGuard — SvelteKit demo

A runnable SvelteKit app on a local SurrealDB, in four beats:

1. **A reactive parameter** — a slider bound to `$state`, passed to a query
   through a thunk. Moving it re-runs the query.
2. **Live updates** — a `<LiveQuery>` plus write buttons on the page, so a
   change is demonstrable from one window.
3. **Nesting** — `<Query>` → `{#each}` → `<LiveQuery>`, one ephemeral
   subscription per row.
4. **Record-level access control** — `DEFINE ACCESS … TYPE RECORD` and
   `PERMISSIONS … WHERE team = $auth.team`, so the *same* query returns
   different rows to different people.

Everything is typed from `schema/schema.surql`. No row or snippet parameter in
`src/routes/+page.svelte` carries a type annotation, and every one of them is
fully typed.

---

## Run it

Two terminals. Run them in this order.

### Terminal 1 — the database

```sh
cd examples/sveltekit
pnpm db
```

Starts SurrealDB in memory on **port 8124** (not 8000 — a demo machine usually
has something on the default already), applies `schema/schema.surql`, and loads
`scripts/seed.surql`. It prints:

```
  SurrealDB ready on ws://127.0.0.1:8124/rpc
  namespace=demo database=demo (root/root)
  seeded 5 people, 6 tickets
```

**Leave it running.** Ctrl-C stops it, and because the store is in memory,
stopping it is also the reset button.

### Terminal 2 — the app

From the repository root, once per checkout:

```sh
pnpm install
pnpm build          # builds packages/*; the example imports their `dist`
```

Then:

```sh
cd examples/sveltekit
pnpm dev
```

Open **<http://localhost:5178>**.

> Vite binds to `localhost`, which on macOS is `::1`. `http://127.0.0.1:5178`
> will *not* answer. Use `localhost`.

The page connects over a WebSocket from the browser and paints rows in well
under a second. There is no server-side rendering (`src/routes/+layout.ts` sets
`ssr = false`), so a `curl` of the page returns an empty shell — that is
expected, not a fault.

### If something is wrong

| Symptom | Fix |
| --- | --- |
| A red bar says **SurrealDB is not running** | It isn't. `pnpm db` in terminal 1. Within two seconds the bar turns green and offers a **Reload** button — press it. (The reload is genuinely necessary: `Surreal.connect()` does not fail when nothing is listening, it *waits*, so a socket opened against a closed port never recovers. The bar exists because no error is ever raised for an `error` snippet to show.) |
| Four spinners, no bar, nothing happens | The health poll is being blocked (an extension, an offline browser). Check `http://127.0.0.1:8124/health` in another tab. |
| `Could not run \`surreal\`` | Install it: `curl -sSf https://install.surrealdb.com \| sh` |
| Port 8124 or 5178 already in use | Something from a previous run. `pkill -f "surreal start"`, and Ctrl-C the old `pnpm dev`. |
| The data has drifted after rehearsing | `pnpm db:seed` in a third terminal puts it back without restarting anything. Reload the page afterwards. |
| Rows are typed `any` in the editor | `pnpm generate` — the registry is stale. It refuses to write while there is an analysis error, so check the output. |

---

## Running the demo

### Beat 1 — the reactive parameter

Drag the slider. The list and the count follow every step.

Say what is happening: `minAge` is plain `$state`. It reaches the query as
`q={() => peopleOver.with({ minAge })}` — **a thunk**. That is the whole
mechanism: the thunk re-runs when its dependencies change, the query key changes
with it, and the cache entry for the old key is dropped.

### Beat 2 — live updates

Press **Add a person**. The row appears in the live list — and note that it
arrives over the subscription, not from the click. The `×` on any row deletes
it.

To make the point properly: **open a second tab on the same URL first**, put
them side by side, and press the button in one. Both update.

### Beat 3 — nesting

Two teams, each with a `<LiveQuery>` mounted *inside* the `{#each}` and
parameterised by that row. Add a person in beat 2 and the matching team grows
while the other does not. Rows sharing a query key share one `LIVE SELECT`;
unmounting a row `KILL`s its subscription.

`recordId(team.id)` is doing the small but load-bearing thing there: a reactive
row is JSON-shaped, so `team.id` is the string `"team:red"`, while a record
*parameter* has to be the SDK's `RecordId` or it matches nothing on the wire.
It ships in `@surrealguard/client` and infers `RecordId<"team">` from the
literal type, with no cast.

### Beat 4 — record-level access control

Start signed out: **root sees all 6 tickets**, because a root user bypasses
table permissions.

- **Sign in as Ada (Red)** → 3 tickets, all `team:red`.
- **Sign in as Grace (Blue)** → 3 tickets, all `team:blue`.
- **Sign out** → 6 again.

The query text never changes. `PERMISSIONS FOR select WHERE team = $auth.team`
on the `ticket` table does all of it, and `DEFINE ACCESS staff ON DATABASE TYPE
RECORD` — SIGNUP, SIGNIN and `DURATION`, in `schema/schema.surql` — is what
makes `$auth` a person. The app contains no authorisation logic at all.

Worth saying out loud, because it is the part that is easy to get wrong: on
sign-in, `src/lib/session.svelte.ts` calls `getQueryClient(db).reset()`. A cache
key is the query text plus its parameters, and `$auth` is in neither — so
without that call, the rows Ada fetched are the rows Grace would render. `reset()`
kills every live subscription (a `LIVE SELECT` captures its permission context
when it opens and cannot be re-pointed), puts every subscribed entry back to
pending, and re-runs it as the new identity.

### The editor moment

`src/lib/queries.ts`, at the bottom: five commented-out one-liners, each
annotated with the exact diagnostic it produces. Uncomment one, save, and the
squiggle appears. The messages in that comment are copied from real runs against
this schema.

```
error[E1002]:   `person` has no field `nmae`        — help: did you mean `name`?
error[E1001]:   `prson` is not a defined table      — help: did you mean `person`?
error[E2004]:   `>` can't combine a `int` and a `string`
error[E4009]:   a live query can't ORDER BY
warning[W4027]: this FETCH does nothing — a DIFF notification is never fetched
```

**Re-comment the line before moving on.** `surrealguard generate` refuses to
write the registry while there is an error, so a stray one makes the app's types
stale at the next regeneration.

The command-line equivalent, if the editor is not cooperating:

```sh
cd examples/sveltekit && surrealguard check
```

---

## Checking it still works

```sh
cd examples/sveltekit
pnpm db          # terminal 1, left running
pnpm verify      # terminal 2
```

`pnpm verify` re-seeds and then drives all four beats headlessly against the
real database, through the same reactive core the components use
(`getQueryClient(db).observe` / `.observeLive` — which is exactly what
`<Query>` / `<LiveQuery>` call). It reads the query texts out of
`src/lib/queries.ts` rather than restating them, so it cannot pass against a
query the app does not run. **It re-seeds, so do not run it while presenting.**

Static checks, from the repository root:

```sh
pnpm -r run typecheck   # 0 errors, including this example
pnpm -r run test        # packages/*; needs no database
```

---

## The other pages

The demo is `/`. Three smaller pages show one idea each, in increasing order of
machinery — they are documentation, not part of the live script:

| Route | What it shows |
| --- | --- |
| `/plain` | `db.query("SELECT …")` in a component, typed, with nothing wrapped around it. |
| `/live` | `preload` in `+page.ts` and `createLive` in the component, without naming the query twice. |
| `/components` | `<Query>` / `<LiveQuery>` over a preloaded payload. |

---

## Files

| | |
| --- | --- |
| `schema/schema.surql` | The schema, including the `DEFINE ACCESS` and the `PERMISSIONS`. What `surrealguard` analyses **and** what the database runs. |
| `scripts/db.mjs` | Starts SurrealDB, applies the schema, seeds. `--seed-only` re-seeds a running one. |
| `scripts/seed.surql` | The data. Deliberately tiny — a large seed was observed to drop live subscriptions. |
| `scripts/verify.mjs` | The headless check described above. |
| `src/lib/queries.ts` | Named queries, and the editor-moment comment block. |
| `src/lib/db.ts` | The one client. Port 8124 lives here and in `scripts/db.mjs`, nowhere else. |
| `src/lib/session.svelte.ts` | Sign-in, and the cache reset that has to follow it. |
| `src/lib/health.svelte.ts` | Demo scaffolding: polls `/health` so "the database is not running" is a sentence rather than a spinner. |
| `src/routes/+page.svelte` | The demo. |
| `src/lib/surrealguard.generated.ts` | Generated; committed on purpose, so a fresh checkout type-checks with no build step. Regenerate with `pnpm generate`. |

## Things not to do live

- **Do not run `pnpm verify` while presenting.** It re-seeds the database
  underneath the open page.
- **Do not leave a diagnostic uncommented** in `src/lib/queries.ts` and then run
  `pnpm generate`; it will refuse and the types stay stale.
- **Do not use `http://127.0.0.1:5178`.** Vite is on `localhost` (`::1`).
- **Do not press "Add a person" more than a handful of times** before beat 3 —
  the point is one row appearing, and a list of twenty makes it harder to see,
  not easier. The `×` buttons undo it.
