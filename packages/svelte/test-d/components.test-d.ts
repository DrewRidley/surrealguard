/**
 * Negative type tests for `<Query>` / `<LiveQuery>`. Compiling this file IS the
 * test: every `@ts-expect-error` line must fail, and the positive control must
 * not. `svelte-check` reports an unused `@ts-expect-error` as an error, so a
 * rule that stops being enforced fails the build rather than passing quietly.
 *
 * The *positive* half — that a `children` snippet parameter is inferred from
 * `q` with no annotation — lives in `Components.svelte`, because a snippet
 * parameter only exists in markup. This file exists because the reverse is
 * true of `@ts-expect-error`: it has no markup syntax, so the rules a component
 * must REFUSE can only be stated here.
 *
 * A Svelte 5 component is a function, and calling it is how a `.ts` file gets
 * at its props type. That is a little closer to the compiler's output than the
 * rest of the suite sits; it is worth it to keep "an unbound query is not
 * renderable" a compiler-enforced rule rather than a documented one.
 */

import type { Snippet } from "svelte";
import LiveQuery from "../src/LiveQuery.svelte";
import Query from "../src/Query.svelte";
import { allMembers, liveMembers, liveTeam, membersOf } from "./fixtures.js";

/** A snippet of whatever shape the call site expects — so a failure below is
 * never merely "your snippet was the wrong type". */
declare function snippet<T extends unknown[]>(): Snippet<T>;
/** Svelte passes this itself; a type test never has one. */
declare const internal: never;

// Positive control. If this ever stops compiling, every negative below is
// passing for the wrong reason.
Query(internal, { q: allMembers, children: snippet() });
LiveQuery(internal, { q: liveMembers, children: snippet() });

// @ts-expect-error a query with unbound params is not renderable — bind it with `.with({…})`
Query(internal, { q: membersOf, children: snippet() });
// @ts-expect-error a live query belongs in <LiveQuery>
Query(internal, { q: liveMembers, children: snippet() });
// @ts-expect-error an unbound live query is not renderable
LiveQuery(internal, { q: liveTeam, children: snippet() });
// @ts-expect-error a one-shot query belongs in <Query>
LiveQuery(internal, { q: allMembers, children: snippet() });
// @ts-expect-error `children` is required: a <Query> with no body renders nothing
Query(internal, { q: allMembers });
// @ts-expect-error no raw-string form; a string in markup cannot be typed yet
Query(internal, { q: "SELECT id FROM member", children: snippet() });
