// ===========================================================================
// TEMPORARY. Delete this file the moment `surrealguard generate` extracts
// queries from Svelte MARKUP.
// ===========================================================================
//
// The demo writes its reads inline, in the attribute, where you are looking:
//
//   <LiveQuery q="SELECT id, name, age, team FROM person WHERE age > {minAge}">
//
// The preprocessor (`@surrealguard/svelte/preprocess`) turns that into the
// static skeleton `… WHERE age > $__host0` plus a bound parameter, and that
// skeleton is what the generated registry is keyed by — which is what makes
// `person.name` a `string` in the snippet with no annotation.
//
// `generate` extracts embedded queries from host-language CALL EXPRESSIONS
// (`db.query("…")`, `defineQuery("…")`), including those inside a `<script>`
// block. It does not yet read markup attributes. So until it does, the two
// skeletons are restated here, and only here, so the registry contains them.
//
// This is exactly the drift the project exists to prevent — two places that
// must agree with nothing enforcing it — which is why it is quarantined in a
// file whose whole purpose is to be deleted, rather than mixed into
// `queries.ts` where it would look like a design decision.

import type { Json, ResultOf, Rows } from "@surrealguard/client";
import { defineQuery } from "$lib/surrealguard.generated";

/** `<LiveQuery q="SELECT id, name, age, team FROM person WHERE age > {minAge}">` */
export const _rosterSkeleton = defineQuery(
  "SELECT id, name, age, team FROM person WHERE age > $__host0",
);

/** `<Query q="SELECT id, title, team FROM ticket">` */
export const _ticketsSkeleton = defineQuery("SELECT id, title, team FROM ticket");

// The row types the snippets in `+page.svelte` annotate themselves with.
//
// They are DERIVED — `ResultOf` reads the same generated registry entry the
// skeletons above key — so nothing here restates a field, and adding a column
// to the query changes them. The annotation is needed at all only because
// `svelte2tsx` (which is what `svelte-check` and the editor's Svelte extension
// use) applies `script` and `style` preprocessors but type-checks the ORIGINAL
// markup, so it sees the attribute as a string and never as the query the
// preprocessor turns it into. When it learns to, delete this file and the two
// annotations with it.

/** Rows of `<LiveQuery q="SELECT id, name, age, team FROM person WHERE age > {minAge}">`. */
export type PersonRows = Json<
  Rows<ResultOf<"SELECT id, name, age, team FROM person WHERE age > $__host0">>
>;

/** Rows of `<Query q="SELECT id, title, team FROM ticket">`. */
export type TicketRows = Json<Rows<ResultOf<"SELECT id, title, team FROM ticket">>>;
