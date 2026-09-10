<!--
  Type-level test for `<Query>` / `<LiveQuery>`. **Compiling this file is the
  test** — `svelte-check` over this package's tsconfig is what runs it, exactly
  as it runs `test-d/queries.test-d.ts`.

  What is under test is inference through markup: a `children` snippet whose
  parameter is written with NO annotation must still be the row type of the
  query passed to `q`. `exact<…>()` is invariant, so a parameter that degrades
  to `any` (the failure mode a `satisfies` check would miss entirely) or to
  `unknown` fails to compile.

  Delete `generics="R"` from `src/Query.svelte` and every `expect*` call below
  reports "Expected 2 arguments, but got 1".
-->
<script lang="ts">
  import type { Preloaded, RecordId } from "@surrealguard/client";
  import { LiveQuery, Query } from "../src/index.js";
  import { exact } from "./expect.js";
  import {
    allMembers,
    liveMember,
    liveMembers,
    liveTeam,
    memberCount,
    membersOf,
    type MemberJson,
  } from "./fixtures.js";

  // Instantiated here so markup calls carry no type-argument syntax.
  const expectRows = exact<MemberJson[]>();
  const expectNamed = exact<Array<{ id: `member:${string}`; name: string }>>();
  const expectCount = exact<number>();
  const expectRetry = exact<() => Promise<void>>();
  const expectError = exact<import("@surrealguard/client").SurrealGuardError>();

  let {
    team,
    id,
    seed,
  }: {
    team: string;
    id: RecordId<"member">;
    // What `preload(db, allMembers)` hands a route through `load`.
    seed: Preloaded<MemberJson[]>;
  } = $props();
</script>

<!-- 1. A one-shot query: the snippet parameter is the unwrapped result. -->
<Query q={allMembers}>
  {#snippet children(members)}
    {@const _rows = expectRows(members)}
    {#each members as member (member.id)}{member.name}{member.age}{/each}
  {/snippet}
</Query>

<!-- 2. `Rows<R>` unwrapping is preserved: a scalar result stays a scalar. -->
<Query q={memberCount}>
  {#snippet children(total)}
    {@const _count = expectCount(total)}
    {total.toFixed(0)}
  {/snippet}
</Query>

<!-- 3. The thunk form keeps params reactive and keeps the row type. -->
<Query q={() => membersOf.with({ team })}>
  {#snippet children(members)}
    {@const _named = expectNamed(members)}
    {members.length}
  {/snippet}
</Query>

<!-- 4. `loading` and `error` snippets, with the retry `<Query>` hands them. -->
<Query q={allMembers}>
  {#snippet children(members)}{members.length}{/snippet}
  {#snippet loading()}loading{/snippet}
  {#snippet error(cause, retry)}
    {@const _cause = expectError(cause)}
    {@const _retry = expectRetry(retry)}
    <button onclick={retry}>{cause.message}</button>
  {/snippet}
</Query>

<!-- 5. A live query's snippet gets an ARRAY of rows — never the live `Uuid`. -->
<LiveQuery q={liveMembers}>
  {#snippet children(rows)}
    {@const _live = expectRows(rows)}
    {#each rows as row (row.id)}{row.name}{/each}
  {/snippet}
  {#snippet error(cause)}
    {@const _liveCause = expectError(cause)}
    {cause.message}
  {/snippet}
</LiveQuery>

<!-- 6. Nesting: an inner per-row live query inside an outer result, which is
     the pattern the components exist for. Both snippet parameters are inferred,
     and the inner query is parameterised by a field of the outer row. -->
<Query q={allMembers}>
  {#snippet children(members)}
    {@const _outer = expectRows(members)}
    {#each members as member (member.id)}
      <LiveQuery q={liveMember.with({ id: member.id as unknown as RecordId<"member"> })}>
        {#snippet children(rows)}
          {@const _inner = expectNamed(rows)}
          {rows[0]?.name}
        {/snippet}
      </LiveQuery>
    {/each}
  {/snippet}
</Query>

<!-- 7. A `Preloaded` payload carries the row type across the SSR boundary, so
     the snippet is typed with the query text named NOWHERE in this file. Both
     directly and through a thunk, which is what a client-side navigation that
     replaces `data` needs. -->
<Query q={seed}>
  {#snippet children(members)}
    {@const _seeded = expectRows(members)}
    {members.length}
  {/snippet}
</Query>
<Query q={() => seed}>
  {#snippet children(members)}
    {@const _seededThunk = expectRows(members)}
    {members.length}
  {/snippet}
</Query>

<!-- 8. `"skip"`, direct and through a thunk, keeps the row type. -->
<LiveQuery q="skip">
  {#snippet children(rows)}{rows.length}{/snippet}
</LiveQuery>
<LiveQuery q={() => (team ? liveTeam.with({ team }) : "skip")}>
  {#snippet children(rows)}
    {@const _skip = expectNamed(rows)}
    {rows.length}
  {/snippet}
</LiveQuery>

{id}
