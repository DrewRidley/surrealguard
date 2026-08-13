<!--
  `<Query>` — a one-shot query, rendered declaratively.

  It is a thin wrapper over `createQuery`: the fetching, the shared cache, the
  refcounted teardown all still live in the reactive core. What the component
  adds is that a route can express "run this, show a spinner, show an error,
  render the rows" in markup, with the row type flowing into the snippet
  parameter and no annotation at the call site.

  ```svelte
  <Query q={allPeople}>
    {#snippet children(people)}
      {#each people as person (person.id)}<li>{person.name}</li>{/each}
    {/snippet}
    {#snippet loading()}<p>Loading…</p>{/snippet}
    {#snippet error(cause, retry)}
      <p>{cause.message}</p><button onclick={retry}>Retry</button>
    {/snippet}
  </Query>
  ```

  `q` takes the same `Source<Q>` the primitives take — a query value, a thunk
  returning one (reactive params), or `"skip"` — and, because the prop itself is
  read through a thunk below, replacing it re-resolves and re-subscribes.

  There is deliberately no `q="SELECT …"` string form. A string in a markup
  attribute cannot be typed until query extraction reads Svelte markup, and a
  prop that yields `unknown` is the permissive overload this project refuses
  everywhere else.

  Fallbacks when a snippet is absent: no `loading` renders nothing (a spinner is
  the app's decision, not the library's); no `error` **throws**, so the failure
  reaches `<svelte:boundary>` or SvelteKit's error page instead of a page that
  is silently, permanently blank.
-->
<script lang="ts" generics="R">
  import type { Snippet } from "svelte";
  import type {
    Bound,
    Json,
    Preloaded,
    Rows,
    SurqlQuery,
    SurrealGuardClient,
    SurrealGuardError,
  } from "@surrealguard/client";
  import { createQuery } from "./queries.svelte.js";
  import { resolveSource, type Source } from "./source.js";

  let {
    q,
    client,
    children,
    loading,
    error,
  }: {
    /**
     * The query to run: a bound `SurqlQuery`, a `Preloaded` payload, a thunk of
     * either, or `"skip"`.
     *
     * A **string** is the inline form — `q="SELECT … WHERE age > {minAge}"` —
     * which `@surrealguard/svelte/preprocess` rewrites into the thunk-plus-parts
     * shape before the compiler sees it. It is accepted here only so the
     * attribute type-checks; a string that actually arrives at runtime means the
     * preprocessor is not installed, and it throws saying so.
     */
    q: Source<SurqlQuery<R, Bound> | Preloaded<Json<Rows<R>>>> | string;
    /** Override the context client (tests, a second connection). */
    client?: SurrealGuardClient;
    /** Rendered with the result once it is available. */
    children: Snippet<[Json<Rows<R>>]>;
    /** Rendered while the first result is outstanding. */
    loading?: Snippet<[]>;
    /** Rendered on failure, with a retry. Omit it and the error is thrown instead. */
    error?: Snippet<[SurrealGuardError, () => Promise<void>]>;
  } = $props();

  // Through a thunk, so replacing the `q` prop re-resolves the source: the old
  // query's subscription is dropped and the new one opened, which is what makes
  // a `<Query>` inside a keyed `{#each}` follow its row.
  //
  // `client` is deliberately read once: `createQuery` resolves it (or falls
  // back to context) at construction, so a closure would buy nothing. Swapping
  // connections means a new component, keyed.
  // svelte-ignore state_referenced_locally
  const handle = createQuery<R>(() => resolveSource(q), { client });

  const failure = $derived(handle.error);

  // An error no `error` snippet claims is thrown rather than swallowed, so it
  // reaches a `<svelte:boundary>` or SvelteKit's error page. It is raised from
  // an effect, not from a derived the template reads, so the throw happens
  // *after* the render pass rather than in the middle of tearing it down.
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
  {@render children(handle.data as Json<Rows<R>>)}
{:else}
  {@render loading?.()}
{/if}
