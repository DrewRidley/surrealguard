<!--
  `<LiveQuery>` — a live query, rendered declaratively.

  A thin wrapper over `createLive`, and separate from `<Query>` on purpose: a
  `live` boolean prop would make one component's result type depend on a flag,
  and "is this array reconciled or fetched once?" is worth reading off the tag.

  ```svelte
  <Query q={allPeople}>
    {#snippet children(people)}
      {#each people as person (person.id)}
        <LiveQuery q={livePerson.with({ id: person.id })}>
          {#snippet children(rows)}{rows[0]?.name}{/snippet}
        </LiveQuery>
      {/each}
    {/snippet}
  </Query>
  ```

  Nesting one per row is the intended pattern, so the mount/unmount cost is the
  design constraint. It is cheap in both directions: mounting joins the
  refcounted cache entry for that key (N components on one key share one `LIVE
  SELECT`), and unmounting drops the last reference, which `KILL`s the
  subscription immediately — `createSubscriber` tears down as soon as nothing
  reads it, and a destroyed component reads nothing. A row scrolled out of view
  therefore stops costing a subscription.

  The snippet receives the reconciled **rows**, never the live-query `Uuid`. The
  uuid is a subscription handle, not a payload; a component that renders one has
  nothing useful to render. It is not exposed.

  Fallbacks match `<Query>`: absent `loading` renders nothing, absent `error`
  throws so the failure surfaces.
-->
<script lang="ts" generics="Q extends LiveSource">
  import type { Snippet } from "svelte";
  import type {
    Bound,
    Json,
    ParamsOf,
    Preloaded,
    SurqlLive,
    SurrealQLAnalyzerClient,
    SurrealQLAnalyzerError,
  } from "@surrealdb/analyzer-client";
  import { sgTextLive } from "./inline.js";
  import { createLive } from "./queries.svelte.js";
  import { resolveSource, type Source } from "./source.js";
  import type { LiveRow, LiveRows, LiveSource } from "./prop.js";

  // One type parameter, inferred off `q` — see `./prop.ts` for why a second one
  // read through a conditional cannot work.
  type Row = LiveRow<Q>;

  let {
    q,
    params,
    client,
    children,
    loading,
    error,
  }: {
    /**
     * The live query: the text of a registered one, a bound `SurqlLive`, a
     * `Preloaded` payload, a thunk of either, or `"skip"`. The text is a plain
     * `SELECT`; the client prefixes `LIVE` when it opens the subscription.
     */
    q: Q;
    /** Bound parameters, when `q` is text that names some. */
    params?: Q extends string ? ParamsOf<Q> : never;
    /** Override the context client (tests, a second connection). */
    client?: SurrealQLAnalyzerClient;
    /** Rendered with the reconciled rows. Always an array, so no `?? []`. */
    children: Snippet<[LiveRows<Q>]>;
    /** Rendered until the seeding `SELECT` resolves. */
    loading?: Snippet<[]>;
    /** Rendered on failure. Omit it and the error is thrown instead. */
    error?: Snippet<[SurrealQLAnalyzerError]>;
  } = $props();

  // Through a thunk: replacing `q` kills the old `LIVE SELECT` and opens the new
  // one, which is what lets a row's subscription follow the row.
  //
  // Read once on purpose — see `<Query>`: the client is resolved at
  // construction, so a closure over it would change nothing.
  // svelte-ignore state_referenced_locally
  const handle = createLive<Row>(() => {
    if (typeof q === "string" && q !== "skip") {
      return sgTextLive(q, params as Record<string, unknown> | undefined) as SurqlLive<Row, Bound>;
    }
    return resolveSource(q as Source<SurqlLive<Row, Bound>>) as SurqlLive<Row, Bound>;
  }, { client });

  const failure = $derived(handle.error);

  // Unclaimed errors are thrown, from an effect — see `<Query>`.
  $effect(() => {
    if (failure && !error) throw failure;
  });

  // A live handle's `data` is `[]` while pending, which is indistinguishable
  // from a query that really has no rows — so gate on `status`, not on length,
  // and an empty result renders `children([])` rather than a stuck spinner.
  const ready = $derived(handle.status === "success");
</script>

{#if failure}
  {@render error?.(failure)}
{:else if ready}
  {@render children(handle.data)}
{:else}
  {@render loading?.()}
{/if}
