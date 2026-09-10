/**
 * Type-level assertion helpers usable from **markup**.
 *
 * `test-d/queries.test-d.ts` asserts with `Expect<Equal<…>>` aliases, which is
 * the right shape in a `.ts` file. A snippet parameter is not visible there: it
 * only exists inside `{#snippet children(rows)}`, in a `.svelte` file, where a
 * type alias cannot be declared and `@ts-expect-error` has no syntax. So the
 * assertion has to be an *expression*, and `{@const}` is where it goes.
 *
 * Assignability is not a strong enough check here. `any` is assignable to
 * everything, so `rows satisfies Person[]` passes on exactly the degradation
 * these tests exist to catch. `exact` is invariant instead.
 */

/** Exact (invariant) type equality. */
export type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;

export type Expect<T extends true> = T;

/**
 * `exact<Expected>()(value)` compiles **only** when `value`'s type is exactly
 * `Expected` — `any`, `unknown`, a widened union or a missing field all fail.
 *
 * The rest-parameter guard is what turns a mismatch into an error: when the
 * types differ, the call demands a second argument of type `never`, which no
 * call site can supply. Instantiate it in `<script>` and call the result in
 * markup, so no type-argument syntax has to survive the template parser.
 */
export function exact<Expected>() {
  return <Actual>(
    _value: Actual,
    ..._guard: Equal<Actual, Expected> extends true ? [] : [never]
  ): void => {};
}
