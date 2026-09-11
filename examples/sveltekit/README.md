# SurrealQL Analyzer — SvelteKit demo

One page. Every query is written **in the markup**, where you are looking when
you want to change it:

```svelte
<LiveQuery q="SELECT id, name, age, team FROM person WHERE age > {minAge}">
```

Move the slider and it re-runs. Add someone and the row arrives over a live
subscription. Sign in as someone else and the *same* ticket query returns
different rows, because SurrealDB's `PERMISSIONS` say so.

The page holds no data of its own. There is no array of people, no list of
teams, no counter — the three things on screen are three `<Query>`s against the
schema, right down to the team dropdown in the add form. The only `$state` in
`+page.svelte` is what the human is typing: the slider, and the form's name,
age and team.

`{minAge}` is **not** string interpolation. See
[What `{minAge}` actually compiles to](#what-minage-actually-compiles-to).

---

## Run it

One install, then two terminals, in this order.

### The CLI, once per checkout

```sh
cd ../..            # the repository root
cargo install --path crates/cli --force
```

The demo needs a `surrealql-analyzer` built from **this** branch. A CLI older than
`0d3229f` reads the `<script>` block but not the markup, so the query in the
`q=` attribute is invisible to it: `surrealql-analyzer check` says "no issues found"
however wrong the attribute is (that is demo beat 4), and `pnpm generate`
writes a registry with the inline queries missing. Neither fails loudly, which
is exactly why this step is here rather than in the troubleshooting table.

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
| `pnpm generate` drops the inline queries, or breaking the `q=` attribute reports nothing | The `surrealql-analyzer` on your `PATH` predates markup extraction. `cargo install --path crates/cli --force`, from the repository root. |

---

## Running the demo

**1. The slider.** Drag it. The list follows every step.

The line to point at is the `q=` attribute. It is SurrealQL, it is typed from
`schema/schema.surql`, and SurrealQL Analyzer reports a mistake in it *on that line*.

**2. Add a person.** Type a name, pick an age, pick a team, press the button.
The row arrives over the subscription, not from the click. Open a second tab
side by side first and add in one — both update. The `×` on a row deletes it.

The team dropdown is worth a sentence of its own: it is a third `<Query>`,
`SELECT id, name FROM team`. Its options *are* rows — so there is no list of
teams in the page to fall out of date with the database. Create a third team and
it is in the dropdown on the next load, with nothing in `src/` edited. (A
one-shot `<Query>`, not a `<LiveQuery>`: teams do not change while you present.)

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
    --> src/routes/+page.svelte:118:49
    |
118 |   <LiveQuery q="SELECT id, name, age, team FROM persn WHERE age > {minAge}">
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

**Re-comment the line before moving on.** `surrealql-analyzer generate` refuses to
write the registry while there is an error.

The command-line equivalent: `surrealql-analyzer check`, from this directory.

---

## The schema

`schema/schema.surql` is both what SurrealQL Analyzer analyses and what `scripts/db.mjs`
applies to the running database, so the schema the types come from and the
schema the demo runs on cannot drift. It carries no comments — this section is
where its reasoning lives.

Three tables, deliberately: a team, the people on it, and the tickets those
people work. An audience reads that in one glance.

**`person` is `PERMISSIONS FULL`, on purpose.** The roster is public in this
demo, so the slider, the live updates and the add form behave identically
whoever is signed in. `team` is open for the same reason: the picker has to keep
offering both teams after you sign in as Ada. The access-control beat lives on
`ticket`, where it is the only thing changing and therefore the only thing to
look at.

**`email` and `password` are `option<>`** because not everyone in the roster has
a login — `alan`, `katherine` and `barbara` in `scripts/seed.surql` do not — and
because it keeps "add a person" a three-parameter write. Make `email` required
instead and SurrealQL Analyzer says so immediately, on the `CREATE` in
`src/lib/queries.ts:17` and on the three seeded people who have no login:

```
error[E2034]: `email` must be set when creating a `person`
  = help: `email` is `string` with no `DEFAULT`, so every create must provide it
```

**`password` is `PERMISSIONS NONE`** so the hash never leaves the database: no
record user can select it. `crypto::argon2::compare` still sees it, because the
`SIGNIN` query runs with full access rather than as the user signing in.

**`ticket` is the access-control beat.** `FOR select WHERE team = $auth.team`
means a record user sees only their own team's tickets; a root user bypasses
table permissions and sees all six. Signing in as someone else visibly changes
the rows, and nothing in the query text says so — `$auth` does. Writes are
`NONE`: the demo never creates a ticket.

**`DEFINE ACCESS staff … TYPE RECORD`** is the authentication, written in
SurrealQL rather than in the app. `db.signin({ access: "staff", variables: {
email, password } })` runs the `SIGNIN` query, and whatever record it returns
becomes `$auth` for that session. There is no authorisation logic in
`src/`.

---

## What `{minAge}` actually compiles to

Svelte compiles an interpolated attribute to string concatenation. Left alone,
`<Query>` would receive a finished string with the value already spliced into
the query text — a SurrealQL injection for a string value, and a brand-new query
text (so a brand-new cache entry) on every keystroke for a number.

So `@surrealdb/analyzer-svelte/preprocess` catches the attribute before the compiler
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
a static skeleton for the registry to key and for SurrealQL Analyzer to analyse. The
thunk is what keeps it reactive — `Source<Q>` resolves it inside a tracking
context, so `minAge` is read there.

It is enabled in `svelte.config.js`:

```js
import { surrealql-analyzer } from "@surrealdb/analyzer-svelte/preprocess";
export default { preprocess: [surrealqlAnalyzer(), vitePreprocess()] };
```

Leave it out and nothing silently misbehaves: `<Query>` throws with a message
telling you to add it.

### One stopgap left, marked and temporary

`src/lib/inline-registry.ts` used to hold two. The first is gone: `surrealql-analyzer
generate` now keys an interpolated attribute with the hole spelled `$__host0`,
which is exactly what the preprocessor emits, so the registry entry is looked up
by the same bytes that run and no query text is restated anywhere in the
example. The file no longer contains a single query string.

What remains is that **`svelte2tsx` type-checks the original markup**.
`svelte-check` and the editor's Svelte extension apply `script` and `style`
preprocessors but not `markup` ones, so they see the attribute as a string and
never as the query it becomes. That is why each `{#snippet children(…)}` carries
an annotation, and why the three row types live in that file. The types are
still *derived* — `ResultOf` reads the generated registry entry the attribute
keys, and `person.nope` is a compile error — but the annotation should not have
to be there. It goes away when `svelte2tsx` learns to apply markup
preprocessors, and the file goes with it.

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
| `src/routes/+page.svelte` | The demo. All three reads are in it, in the markup. |
| `schema/schema.surql` | The schema, the `PERMISSIONS` and the `DEFINE ACCESS`. What SurrealQL Analyzer analyses **and** what the database runs. Its reasoning is [above](#the-schema). |
| `scripts/db.mjs` | Starts SurrealDB, applies the schema, seeds. `--seed-only` re-seeds a running one. |
| `scripts/seed.surql` | The data. Deliberately tiny — a large seed was observed to drop live subscriptions. |
| `scripts/verify.mjs` | The headless check above. |
| `src/lib/queries.ts` | The two writes, and the editor-moment comment block. |
| `src/lib/db.ts` | The one client. Port 8124 lives here and in `scripts/db.mjs`, nowhere else. |
| `src/lib/session.svelte.ts` | Sign-in, and the cache reset that has to follow it. |
| `src/lib/inline-registry.ts` | The row types the snippets annotate themselves with, and nothing else. The stopgap above. Delete on sight, once you can. |
| `src/lib/surrealql-analyzer.generated.ts` | Generated; committed on purpose, so a fresh checkout type-checks with no build step. `pnpm generate`. |

## Things not to do live

- **Do not run `pnpm verify` while presenting.** It re-seeds under the open page.
- **Do not leave a diagnostic uncommented** and then run `pnpm generate`.
- **Do not use `http://127.0.0.1:5178`.** Vite is on `localhost`.
- **Do not delete Ada or Grace** with the `×` buttons — they are the two logins.
  `pnpm db:seed` puts them back.
