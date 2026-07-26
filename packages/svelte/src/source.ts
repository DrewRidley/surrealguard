/**
 * `Source<Q>` — how a query reaches a reactive primitive.
 *
 * ```ts
 * createLive(livePeople)                                       // static
 * createLive(() => liveTeam.with({ team: page.params.team }))  // reactive
 * createLive(() => enabled ? liveTeam.with({ team }) : "skip") // conditional
 * ```
 *
 * The thunk form is the whole point. `LiveQueryOptions.params` in 0.4 was read
 * **once**, at construction, so a query parameterised on `$derived` state could
 * not re-run — navigating to another team did nothing at all. That is the
 * pre-runes shape: `@tanstack/svelte-query` cut a whole major version (v6) to
 * move from `StoreOrVal<T> = T | Readable<T>` to `Accessor<T> = () => T`, and
 * `convex-svelte` independently landed on the same `T | (() => T)`. The
 * convention is settled, so this adopts it rather than inventing.
 *
 * `"skip"` is Convex's conditional-query sentinel. Without it, "don't run this
 * query yet" has no expression in a thunk-based API short of not rendering the
 * component.
 */

/** A query, a thunk returning one, or the skip sentinel. */
export type Source<Q> = Q | (() => Q | "skip") | "skip";

/** Resolve a source to a query, or `"skip"`. Call inside a tracking context. */
export function resolveSource<Q>(source: Source<Q>): Q | "skip" {
  if (source === "skip") return "skip";
  return typeof source === "function" ? (source as () => Q | "skip")() : source;
}
