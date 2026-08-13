// ===========================================================================
// TEMPORARY. Delete this file the moment `svelte2tsx` applies MARKUP
// preprocessors.
// ===========================================================================
//
// The demo writes its reads inline, in the attribute, where you are looking:
//
//   <LiveQuery q="SELECT id, name, age, team FROM person WHERE age > {minAge}">
//
// The preprocessor (`@surrealguard/svelte/preprocess`) turns that into the
// static skeleton `… WHERE age > $__host0` plus a bound parameter, and that
// skeleton is what the generated registry is keyed by. `surrealguard generate`
// reads the markup attribute and keys the entry with the same `$__host0` the
// preprocessor emits, so nothing here has to restate a query any more — the
// three types below are looked up, not declared.
//
// What is still needed is the ANNOTATION at each `{#snippet children(…)}`.
// `svelte2tsx` — which is what `svelte-check` and the editor's Svelte
// extension use — applies `script` and `style` preprocessors but type-checks
// the ORIGINAL markup, so it sees the attribute as a plain string and never as
// the query the preprocessor turns it into. Until it learns to, the snippet
// parameter needs a type, and these are those types.
//
// They are DERIVED: `ResultOf` reads the same generated registry entry the
// attribute keys, so nothing here restates a field, and adding a column to a
// query changes them. `person.nope` is a compile error. When `svelte2tsx`
// applies markup preprocessors, delete this file and the annotations with it.
//
// `scripts/verify.mjs` catches any drift: it asserts the text the preprocessor
// produces for this page, and runs it.

import type { Json, ResultOf, Rows } from "@surrealguard/client";

/** Rows of `<LiveQuery q="SELECT id, name, age, team FROM person WHERE age > {minAge}">`. */
export type PersonRows = Json<
  Rows<ResultOf<"SELECT id, name, age, team FROM person WHERE age > $__host0">>
>;

/** Rows of `<Query q="SELECT id, title, team FROM ticket">`. */
export type TicketRows = Json<Rows<ResultOf<"SELECT id, title, team FROM ticket">>>;

/** Rows of `<Query q="SELECT id, name FROM team">` — the add form's picker. */
export type TeamRows = Json<Rows<ResultOf<"SELECT id, name FROM team">>>;
