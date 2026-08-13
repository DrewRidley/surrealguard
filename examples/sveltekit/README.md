# SurrealGuard — SvelteKit demo

One page. The query is written **in the markup**, where you are looking when
you want to change it:

```svelte
<LiveQuery q="SELECT id, name, age, team FROM person WHERE age > {minAge}">
```

Move the slider and it re-runs. Press a button and the rows arrive over a live
subscription. Sign in as someone else and the *same* ticket query returns
different rows, because SurrealDB's `PERMISSIONS` say so.

`{minAge}` is **not** string interpolation. See
[What `{minAge}` actually compiles to](#what-minage-actually-compiles-to).

---

## Run it

Two terminals, in this order.

### Terminal 1 — the database

```sh
cd examples/sveltekit
pnpm db
```

SurrealDB, in memory, on **port 8124** (not 8000 — a demo machine usually has
something on the default already). It applies `schema/schema.surql`, loads
`scripts/seed.surql`, and prints:

```
  SurrealDB ready on ws://127.0.0.1:8124/rpc
  namespace=demo database=demo (root/root)
  seeded 5 people, 6 tickets
```

**Leave it running.** Ctrl-C stops it, and since the store is in memory,
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

> Vite binds to `localhost`, which is `::1` on macOS. `http://127.0.0.1:5178`
> will not answer. Use `localhost`.

There is no server-side rendering (`src/routes/+layout.ts` sets `ssr = false`),
so `curl` of the page returns an empty shell. That is expected: the WebSocket is
opened from the browser.

### If something is wrong

| Symptom | Fix |
| --- | --- |
| A red bar says **SurrealDB is not running** | It isn't. `pnpm db` in terminal 1. Within two seconds the bar turns green with a **Reload** button — press it. The reload is genuinely needed: `Surreal.connect()` does not fail when nothing is listening, it *waits*, so a socket opened against a closed port never recovers. |
| `Could not run \`surreal\`` | `curl -sSf https://install.surrealdb.com \| sh` |
| Port 8124 or 5178 in use | `pkill -f "surreal start"`, and Ctrl-C the old `pnpm dev`. |
| The data drifted after rehearsing | `pnpm db:seed` in a third terminal. Reload the page. |
| Rows are `unknown` in the editor | `pnpm generate` — the registry is stale. It refuses to write while there is an analysis error, so read the output. |

---

## Running the demo

**1. The slider.** Drag it. The list follows every step.

The line to point at is the `q=` attribute. It is SurrealQL, it is typed from
`schema/schema.surql`, and SurrealGuard reports a mistake in it *on that line*.

**2. Add a person.** The row arrives over the subscription, not from the click.
Open a second tab side by side first and press the button in one — both update.
The `×` on a row deletes it.

Worth doing: set the slider to 45 first, then add someone. Nothing appears —
the filter is the database's, not the page's — and dropping the slider brings
them in.

**3. Sign in.** Root sees all 6 tickets, because a root user bypasses table
permissions. Ada (Red) sees 3, Grace (Blue) sees the other 3, `root` puts it
back. The query text never changes; `PERMISSIONS FOR select WHERE team =
$auth.team` on the table and `DEFINE ACCESS staff … TYPE RECORD` beside it — both
in `schema/schema.surql` — do all of it. There is no authorisation logic in the
app.

On sign-in, `src/lib/session.svelte.ts` calls `getQueryClient(db).reset()`. That
is not housekeeping: a cache key is the query text plus its parameters, and
`$auth` is in neither, so without it the rows Ada fetched are the rows Grace
would render.

**4. The editor.** Break the query in the attribute. `persn` for `person` gives,
on that line and under that word:

```
error[E1001]: `persn` is not a defined table
   --> src/routes/+page.svelte:85:49
    |
 85 |   <LiveQuery q="SELECT id, name, age, team FROM persn WHERE age > {minAge}">
    |                                                 ^^^^^
    |
    = help: did you mean `person`?
```

Three more, each with the exact message it produces, are commented out at the
bottom of `src/lib/queries.ts`:

```
error[E1002]: `person` has no field `nmae`        — help: did you mean `name`?
error[E2004]: `>` can't combine a `int` and a `string`
error[E4009]: a live query can't ORDER BY
```

**Re-comment the line before moving on.** `surrealguard generate` refuses to
write the registry while there is an error.

The command-line equivalent: `surrealguard check`, from this directory.

---

## What `{minAge}` actually compiles to

Svelte compiles an interpolated attribute to string concatenation. Left alone,
`<Query>` would receive a finished string with the value already spliced into
the query text — a SurrealQL injection for a string value, and a brand-new query
text (so a brand-new cache entry) on every keystroke for a number.

So `@surrealguard/svelte/preprocess` catches the attribute before the compiler
and captures the parts:

```svelte
<LiveQuery q="SELECT id, name, age, team FROM person WHERE age > {minAge}">
```

becomes, pre-compile,

```svelte
<LiveQuery q={() => __sg_live(["SELECT id, name, age, team FROM person WHERE age > ", ""], [minAge])}>
```

which runs

```
text    SELECT id, name, age, team FROM person WHERE age > $__host0
params  { __host0: minAge }
```

One query text whatever the slider says, a real bound parameter on the wire, and
a static skeleton for the registry to key and for SurrealGuard to analyse. The
thunk is what keeps it reactive — `Source<Q>` resolves it inside a tracking
context, so `minAge` is read there.

It is enabled in `svelte.config.js`:

```js
import { surrealguard } from "@surrealguard/svelte/preprocess";
export default { preprocess: [surrealguard(), vitePreprocess()] };
```

Leave it out and nothing silently misbehaves: `<Query>` throws with a message
telling you to add it.

### Two stopgaps, both marked, both temporary

`src/lib/inline-registry.ts` exists for a reason that will expire:

1. **The registry key for an interpolated attribute does not match the runtime
   text yet.** `surrealguard generate` reads the attribute — the diagnostic does
   land on that line — but keys the entry with the hole spelled `${}` where the
   runtime produces `$__host0`, and a registry key has to match byte for byte.
   So the two skeletons are restated there — once — until the two agree.
2. **`svelte2tsx` type-checks the original markup.** `svelte-check` and the
   editor's Svelte extension apply `script` and `style` preprocessors but not
   `markup` ones, so they see the attribute as a string and never as the query
   it becomes. That is why the two snippet parameters carry an annotation. The
   types are still *derived* from the schema — `person.nope` is a compile error
   — but the annotation should not have to be there.

The first goes away when the two spellings agree; the second when `svelte2tsx`
learns to apply markup preprocessors.

---

## Checking it still works

```sh
pnpm db          # terminal 1, left running
pnpm verify      # terminal 2
```

`pnpm verify` runs the preprocessor over `src/routes/+page.svelte`, pulls out the
queries it emitted, and drives them against the real database through the same
reactive core the components use — so it cannot pass against a query the page
does not run. It also proves the parameter is bound rather than spliced, by
matching a name containing both kinds of quote.

**It re-seeds. Do not run it while presenting.**

From the repository root: `pnpm -r run typecheck` and `pnpm -r run test`,
neither of which needs a database.

---

## Files

| | |
| --- | --- |
| `src/routes/+page.svelte` | The demo. Both queries are in it. |
| `schema/schema.surql` | The schema, the `PERMISSIONS` and the `DEFINE ACCESS`. What SurrealGuard analyses **and** what the database runs. |
| `scripts/db.mjs` | Starts SurrealDB, applies the schema, seeds. `--seed-only` re-seeds a running one. |
| `scripts/seed.surql` | The data. Deliberately tiny — a large seed was observed to drop live subscriptions. |
| `scripts/verify.mjs` | The headless check above. |
| `src/lib/queries.ts` | The two writes, and the editor-moment comment block. |
| `src/lib/db.ts` | The one client. Port 8124 lives here and in `scripts/db.mjs`, nowhere else. |
| `src/lib/session.svelte.ts` | Sign-in, and the cache reset that has to follow it. |
| `src/lib/inline-registry.ts` | The two stopgaps above. Delete on sight, once you can. |
| `src/lib/surrealguard.generated.ts` | Generated; committed on purpose, so a fresh checkout type-checks with no build step. `pnpm generate`. |

## Things not to do live

- **Do not run `pnpm verify` while presenting.** It re-seeds under the open page.
- **Do not leave a diagnostic uncommented** and then run `pnpm generate`.
- **Do not use `http://127.0.0.1:5178`.** Vite is on `localhost`.
- **Do not delete Ada or Grace** with the `×` buttons — they are the two logins.
  `pnpm db:seed` puts them back.
