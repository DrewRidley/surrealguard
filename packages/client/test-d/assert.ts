/** Type-level assertion helpers shared by the `test-d` files. */

/** True iff `T` is exactly `any`. Degrading to `unknown` is fine; `any` is not. */
export type IsAny<T> = 0 extends 1 & T ? true : false;

/** Exact (invariant) type equality. */
export type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;

export type Expect<T extends true> = T;
