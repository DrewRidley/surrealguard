<!--
  `<Query>` — a one-shot query, rendered declaratively.

  It is a thin wrapper over `createQuery`: the fetching, the shared cache and the
  refcounted teardown all still live in the reactive core. What the component
  adds is that a route can express "run this, show a spinner, show an error,
  render the rows" in markup, with the row type flowing into the snippet
  parameter and no annotation at the call site.

  ```svelte
  <Query q="SELECT id, name FROM team">
    {#snippet children(teams)}
      {#each teams as team (team.id)}<li>{team.name}</li>{/each}
    {/snippet}
  </Query>

  <Query q="SELECT id, name FROM person WHERE age > $min" params={{ min: minAge }}>
    {#snippet children(people)}…{/snippet}
  </Query>
  ```

  `q` is the query text, and it must be a query the generated registry knows —
  a string that is not in it is a type error naming the remedy, never a silent
  `unknown`. That is what types `children` with no annotation, and it is why the
  prop is not simply `string`.

  Parameters are passed as `params`, spelled the way SurrealQL spells them
  (`$min`). Nothing is interpolated into the text: the text is a constant, so it
  is one cache entry and one registry key however the parameters change, and a
  value can never be spliced into the query.

  `q` also takes the values the primitives take — a `SurqlQuery`, a `Preloaded`
  payload, a thunk of either, or `"skip"` — for a query named elsewhere.

  Fallbacks when a snippet is absent: no `loading` renders nothing (a spinner is
  the app's decision, not the library's); no `error` **throws**, so the failure
  reaches `<svelte:boundary>` or SvelteKit's error page instead of a page that is
  silently, permanently blank.
-->
<script lang="ts" generics="Q extends QuerySource">
  import type { Snippet } from "svelte";
  import type {
    Bound,
    Json,
    ParamsOf,
    Preloaded,
    Rows,
    SurqlQuery,
    SurrealGuardClient,
    SurrealGuardError,
  } from "@surrealguard/client";
  import { sgText } from "./inline.js";
  import { createQuery } from "./queries.svelte.js";
  import { resolveSource, type Source } from "./source.js";
  import type { QueryResult, QueryRows, QuerySource } from "./prop.js";

  // One type parameter, inferred straight off `q`. An earlier attempt kept a
  // separate `R` for the value form and read it through a conditional; the
  // conditional is not an inference site, so `R` never resolved and every
  // snippet fell back to `unknown` — which is the annotation this exists to
  // delete.
  type Result = QueryResult<Q>;

  let {
    q,
    params,
    client,
    children,
    loading,
    error,
  }: {
    /** The query: its text, or a value naming it. */
    q: Q;
    /** Bound parameters, when `q` is text that names some. */
    params?: Q extends string ? ParamsOf<Q> : never;
    /** Override the context client (tests, a second connection). */
    client?: SurrealGuardClient;
    /** Rendered with the result once it is available. */
    children: Snippet<[QueryRows<Q>]>;
    /** Rendered while the first result is outstanding. */
    loading?: Snippet<[]>;
    /** Rendered on failure, with a retry. Omit it and the error is thrown instead. */
    error?: Snippet<[SurrealGuardError, () => Promise<void>]>;
  } = $props();

  // Through a thunk, so replacing `q` — or `params` — re-resolves the source:
  // the old query's subscription is dropped and the new one opened. That is
  // what makes `params={{ min: minAge }}` re-run as `minAge` changes, and what
  // makes a `<Query>` inside a keyed `{#each}` follow its row.
  //
  // `client` is deliberately read once: `createQuery` resolves it (or falls back
  // to context) at construction, so a closure would buy nothing. Swapping
  // connections means a new component, keyed.
  // svelte-ignore state_referenced_locally
  const handle = createQuery<Result>(() => {
    if (typeof q === "string" && q !== "skip") {
      return sgText(q, params as Record<string, unknown> | undefined) as SurqlQuery<Result, Bound>;
    }
    return resolveSource(q as Source<SurqlQuery<Result, Bound>>) as SurqlQuery<Result, Bound>;
  }, { client });

  const failure = $derived(handle.error);

  // An error no `error` snippet claims is thrown rather than swallowed, so it
  // reaches a `<svelte:boundary>` or SvelteKit's error page. It is raised from an
  // effect, not from a derived the template reads, so the throw happens *after*
  // the render pass rather than in the middle of tearing it down.
  $effect(() => {
    if (failure && !error) throw failure;
  });

  // `data` may legitimately be `undefined` on success (`RETURN NONE`), and may
  // legitimately be present while still pending (a `Preloaded` seed, a warm
  // cache entry) — which is exactly the case that must not flash a spinner.
  const ready = $derived(handle.status === "success" || handle.data !== undefined);
</script>

{#if failure}
  {@render error?.(failure, handle.refetch)}
{:else if ready}
  {@render children(handle.data as QueryRows<Q>)}
{:else}
  {@render loading?.()}
{/if}
