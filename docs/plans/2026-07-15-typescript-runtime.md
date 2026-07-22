# SurrealGuard TypeScript Runtime — Design

Status: packages publish-ready pending the actual publish (2026-07-17). The `packages/` pnpm
monorepo — `@surrealguard/client`, `@surrealguard/query`, `@surrealguard/next`,
`@surrealguard/svelte` — builds (tsup → ESM + CJS + `.d.ts`), typechecks, and the query core
has vitest tests (one-shot, live CREATE/UPDATE/DELETE reconcile, refcount share+kill,
dehydrate/hydrate). Publish metadata is set (`exports`→dist, `files:["dist"]`, `publishConfig`,
`repository`, per-package README); `npm pack --dry-run` shows clean tarballs. Codegen now emits
the module augmentation (verified end to end). CI runs a `packages` job (build/typecheck/test).
**Remaining before publish:** run the publish itself; copy the dual LICENSE files into each
package (or a prepublish step) — the SPDX `license` field is set but the text isn't bundled;
optional deeper framework-level (jsdom) integration tests. Dioxus deprioritized per Drew.

## Built so far (`packages/`)

- **`@surrealguard/client`** — the typed client. `SurqlRegistry` base interface (augmented by
  codegen), `RecordId<T>`, the proven single-generic `SurrealGuardClient.query` (result +
  params inferred from the query text; dynamic strings → `unknown`), and `fromSurreal` to
  adapt the SurrealDB JS SDK. Type contract verified against tsc via `test-d`.
- **`@surrealguard/query`** — framework-agnostic core: `QueryClient` with a `(sql, params)`
  cache, reference-counted subscriptions (N subscribers share one live subscription),
  `LIVE SELECT` reconciliation by record `id` (CREATE→append, UPDATE→replace, DELETE→remove),
  and `dehydrate`/`hydrate` for SSR.
- **`@surrealguard/next`** — `useLiveQuery` over `useSyncExternalStore` (seeds from
  `initialData`, subscribes on mount, releases on unmount).
- **`@surrealguard/svelte`** — `liveQuery` returning a `svelte/store` `Readable` (seeds from
  `initial`, auto-releases when the last subscriber leaves).

**Connecting gap (codegen): RESOLVED (2026-07-17).** `render_registry` now emits a module
augmentation — `import type { RecordId, GeoJSON } from "@surrealguard/client"` +
`declare module "@surrealguard/client" { interface SurqlRegistry { "…": { result; params } } }`
— so the package's single-generic `query` resolves each entry. Proven end to end: the codegen
`sample` output dropped into `packages/client/test-d/gen` typechecks, with `@ts-expect-error`
confirming missing-params and unknown-field are rejected.

### Overloading the real SurrealDB client (considered)

Idea: augment the SDK's own `Surreal.query` (keyed by query text/hash) so users type their
existing client with no wrapper. **Soundness constraint:** the SDK's `query` returns
per-statement *wrapped* results (`[rows]`), not `rows` — and type augmentation cannot change
runtime unwrapping. So:
- **Wrapper (chosen):** `SurrealGuardClient` composes a `Connection`; its `query` unwraps
  `result[0]` at runtime, so the clean result type is *truthful*. `fromSurreal(db)` adapts a
  real `Surreal` instance — you keep your connection and wrap it once. Sound + clean.
- **Truthful raw-SDK augmentation:** module-augment `Surreal.query` to return the *wrapped*
  shape (`Promise<[Rows]>`); uses the raw client but forces `(await …)[0]`. Sound but clunky.
- **Subclass + override `query`:** hits TS's return-type covariance rule (override must be
  assignable to the base signature); fragile against the SDK's typing.
- **Per-query hashed methods** don't add soundness — the runtime unwrap is uniform, so they'd
  all behave identically; naming per query is the graphql-codegen model (named operations).

Conclusion: the composition wrapper already delivers "your normal client, typed" via
`fromSurreal`, soundly. A truthful raw-SDK augmentation can be offered as an opt-in later.

---
Original design below.


## Goal

End-to-end typed SurrealQL in **Next.js (App Router)** and **SvelteKit**, where the
developer never has to think about SSR/hydration, live-query subscription lifetimes,
or connection/auth plumbing.

```ts
// The whole ergonomic, top to bottom:
const users = await db.query("SELECT * FROM user");        // fully typed, no wrapper
```
```svelte
<script>
  // SSR-seeded, upgrades to live, auto-unsubscribes on destroy:
  const users = liveQuery("LIVE SELECT * FROM user", { initial: data.users });
</script>
{#each users as user}
  <li>{user.name}</li>
{/each}
```

Decisions locked with Drew: build a **framework-agnostic core first**, then thin
SvelteKit + Next adapters; the live layer does **shared cache + dedup** (N components
sharing a query share one subscription); a custom overloaded `db` is preferred over a
`surql()` wrapper; packages live in a **`packages/` monorepo** in this repo (the
codegen↔runtime contract is the crux — one repo lets CI regenerate types and
typecheck the runtime against them in a single pass).

## Proven core: the type contract

The riskiest question — can a bare `db.query("literal")` resolve the result type AND
enforce params (required when present, forbidden when absent) purely from generated
types — is **validated against real tsc 5.9.3**. The winning shape is a single generic
signature with a conditional arg-tuple and conditional return; a second permissive
`string` overload must NOT exist, or it rescues mis-calls into `unknown` and defeats
param checking.

```ts
// Shipped once by @surrealguard/client (hand-written, stable):
interface SurqlRegistry {}                       // base — augmented by generated file
type ParamsArg<P> = P extends Record<string, never> ? [] : [params: P];

declare class SurrealGuardClient {
  query<Q extends string>(
    query: Q,
    ...args: Q extends keyof SurqlRegistry
      ? ParamsArg<SurqlRegistry[Q]["params"]>
      : [params?: Record<string, unknown>]
  ): Promise<Q extends keyof SurqlRegistry ? SurqlRegistry[Q]["result"] : unknown>;
}
```
```ts
// Emitted by `surrealguard generate` (regenerated on schema/query change):
interface SurqlRegistry {
  "SELECT * FROM user": {
    result: Array<{ id: RecordId<"user">; name: string; age: number }>;
    params: Record<string, never>;
  };
  "SELECT * FROM user WHERE team = $team": {
    result: Array<{ id: RecordId<"user">; name: string }>;
    params: { team: string };
  };
}
```

Verified matrix (positive compiles; each negative rejected):

| Case | tsc |
| --- | --- |
| `db.query("SELECT * FROM user")` — typed, no params allowed | ✅ |
| `db.query("… $team", { team: "red" })` — params required + typed | ✅ |
| dynamic `string` → `unknown` result, optional params | ✅ |
| missing required params | ❌ TS2554 |
| wrong param type | ❌ TS2322 |
| params on a no-param query | ❌ TS2554 |
| unknown result field | ❌ TS2339 |
| field not in projection | ❌ TS2339 |

The `db.surql`…`` tagged-template form stays as the escape hatch for dynamic fragments;
it types as `unknown` (TS cannot infer literal types from template arrays — TS#33304)
but the analyzer/LSP still check it.

## Package layout (`packages/`, pnpm workspace)

- **`@surrealguard/client`** — framework-agnostic. The typed `query()` / `live()`,
  connection management, auth-token handling. Ships the base `SurqlRegistry` + the
  proven `query` signature. Wraps the SurrealDB JS SDK.
- **`@surrealguard/query`** — framework-agnostic cache/dedup/live-store engine (the
  "shared cache + dedup" tier). A `QueryCache` keyed by (canonical sql + stable params),
  reference-counted live subscriptions, and SSR dehydrate/hydrate. Exposes a minimal
  observable that adapters wrap.
- **`@surrealguard/sveltekit`** — Svelte 5 runes / stores over the core; `load` helpers.
- **`@surrealguard/next`** — React hooks over the core; RSC helpers + hydration boundary.

The generated `surrealguard.d.ts` is consumed by all of them via global interface merge.

## Cache + dedup + live model

- **Query key** = canonical(sql) + stable-serialized params.
- **QueryCache entry** = `{ data, status, subscribers, liveHandle? }`.
- **Reference counting**: the first subscriber to a live key opens exactly one
  `LIVE SELECT`; the last to leave sends `KILL`. N components → 1 subscription.
- **One-shot dedup**: concurrent `query()` calls for the same key share a single
  in-flight promise.
- **Invalidation**: v1 = explicit `invalidate(key)` + refetch after mutations.
  Optimistic updates + normalized entity cache are a v2 concern.

## SSR + hydration ("don't think about it", part 1)

- **Server**: queries run in `load` (SvelteKit) or a Server Component (Next) during the
  request; results collected into a dehydrated snapshot keyed by query key.
- **Transport**: snapshot serialized alongside the page payload.
- **Client**: cache hydrates from the snapshot (no refetch flash). If the query is LIVE,
  the store is seeded from hydrated data, then opens a subscription to upgrade in place.
- **Auth**: `createServerClient(event/cookies)` carries the session token per request;
  the browser client owns its own connection.

## Live lifetime ("don't think about it", part 2)

Division of responsibility (Drew): the **analyzer does nothing live-specific** — a
`LIVE SELECT`'s row type is just the normal SELECT analysis of its projection. The
**host adapter** owns all the transparency: run once to get the UUID, subscribe, apply
notifications, `KILL` on destroy. No keyword-sniffing in the engine.

**Separate explicit method, not a unified `query`.** `query` stays one-shot; live gets
its own entry point (working name `queryLive` / `db.live(...)`; hook `useLiveQuery`,
Svelte `liveQuery`). Auto-detecting `LIVE` inside `query()` is rejected — it would make
one method return two very different things. The live method: (1) runs the `LIVE SELECT`
to get the **UUID**, (2) subscribes wired into `onMount`/`useEffect`, (3) notifications
update the reactive value, (4) `KILL` on **destroy**.

**"Updates propagate" = apply diffs by record id.** SurrealDB live notifications arrive
as `{ action: CREATE | UPDATE | DELETE, result: row }`, not a fresh array. The store
seeds from initial data (SSR or first fetch) and reconciles per notification: CREATE →
append, UPDATE → replace the row whose `id` matches, DELETE → remove by `id`. Every live
row type therefore needs an `id` to key on (SurrealGuard types record ids, so this holds).

**Lifetime under dedup.** On mount: refcount++ (attach to the shared subscription for
the key, or open it). On destroy: refcount--; at zero, one `KILL`. N components sharing a
live query share one subscription and one reconciled array. Socket drop → reconnect +
resubscribe. All in `@surrealguard/query`; adapters are thin bindings.

**Destroy, not visibility.** Cancel on unmount (simplest, correct, no "why didn't it
update" surprises). Pausing off-screen (`IntersectionObserver`) is an opt-in later tier.

### Engine dependency: LIVE notification row kind

The statement's own response type stays `Kind::Uuid` — that is what `db.query("LIVE
SELECT …")` genuinely returns, and `KILL $id` depends on `$id` being a uuid, so this is
kept for language correctness. `analyze_live_select`
(`crates/workspace/src/analyzer/data/live_select.rs`) today returns `Kind::Uuid` and only
checks the table reference; it does **not** compute the projection's row type. The live
method needs that row type so its reactive container is the projected row array (e.g.
`Array<{ id: RecordId<"user">; name: string; age: number }>`). **Task**: run the same
projection/VALUE/graph inference a normal `SELECT` gets against the table and expose it as
a side output (the statement type stays `Uuid`); codegen emits a separate live registry
entry keyed off that row type, consumed only by `queryLive`. Reuses existing SELECT
machinery — no new inference, no live-specific analysis.

**Types are structural and per-query — never named.** There is no `User` interface. Every
result/row type is the anonymous object shape inferred from that specific query's
projection, mapped straight from the analyzer's `Kind` (as `registry.rs` already does:
`Array<{ name: string }>`). `SELECT * FROM user` → the full row shape; `SELECT name FROM
user` → `Array<{ name: string }>`. Consequence for live: id-keyed reconciliation needs
`id` in the projected shape, so a `LIVE SELECT` whose projection omits `id` either can't
reconcile or the client must request `id` implicitly — flag as a lint/decision.

## Framework surfaces

**SvelteKit**
```ts
// +page.server.ts
export const load = async ({ locals }) => {
  const users = await createServerClient(locals).query("SELECT * FROM user");
  return { users };
};
```
```svelte
<!-- +page.svelte -->
<script>
  export let data;
  const users = liveQuery("LIVE SELECT * FROM user", { initial: data.users });
</script>
{#each users as user}<li>{user.name}</li>{/each}
```

**Next (App Router)**
```tsx
// server component
const users = await createServerClient(cookies()).query("SELECT * FROM user");
return <UserList initialData={users} />;

// client component
const users = useLiveQuery("LIVE SELECT * FROM user", { initialData });
```

## Diagnostics coupling (engine side)

- **`db.query("…")` + `db.surql`…`` inline diagnostics**: extend the embed extractor's
  sink recognition (`is_surql_tag`) to also match a **configurable list of call names**
  (`db.query`, `query`, …) so inline squiggles land on these forms, not only on `surql`.
  Small, bounded change in `crates/embed`.
- **Svelte markup extraction**: only needed if we want `{#each query("…")}` *literally
  in markup* to be diagnosed. `script_blocks()` currently scans `<script>` only. The
  runtime `liveQuery(...)` + `{#each users}` form avoids this, so markup extraction is a
  later, optional item.
- **LSP-driven regeneration**: today `surrealguard generate` is a manual batch step.
  Debounced regeneration on `didChange` is what makes the *types* feel as instant as the
  diagnostics already are. Tracked as a separate engine task.

## Rollout order

1. **codegen** (Rust, small): emit the `SurqlRegistry` augmentation + ship the proven
   client signature shape; cover both `db.query("…")` and `db.surql`…``.
2. **`@surrealguard/client`**: typed `query`/`live` + connection/auth over the SDK.
3. **`@surrealguard/query`**: cache/dedup/live/dehydrate core.
4. **`@surrealguard/sveltekit`**, then **`@surrealguard/next`** (thin over the core).
5. **embed**: `db.query`/`db.surql` sink extraction for inline diagnostics.
6. **LSP regen**: debounced `.d.ts` regeneration for the instant-types loop.

## Open questions

- SurrealDB JS SDK: confirm the live-query API surface (`live()` / `subscribeLive`) and
  the minimum version; confirm auth/token handoff for server vs browser.
- Monorepo tooling: pnpm workspaces + turbo vs. changesets for release.
- Auth model specifics per framework (cookie/session → token).
```
