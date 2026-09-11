/**
 * `createMutation` — a write, plus the invalidation that has to follow it.
 *
 * 0.4 had no mutation helper at all, so writes bypassed the package entirely
 * and nothing in the cache ever learned they had happened: a non-live cached
 * query stayed stale forever.
 *
 * ```svelte
 * <script lang="ts">
 *   import { createMutation } from "@surrealdb/analyzer-svelte";
 *   import { addPerson, allPeople, livePeople } from "$lib/queries";
 *
 *   const add = createMutation(addPerson, { invalidates: [allPeople, livePeople] });
 * </script>
 *
 * <button onclick={() => add.mutate({ name: "ada", joined: new Date() })}
 *         disabled={add.pending}>Add</button>
 * {#if add.error}<p class="error">{add.error.message}</p>{/if}
 * ```
 */

import {
  SurrealQLAnalyzerError,
  type AnyQuery,
  type ParamsArg,
  type Rows,
  type SurqlQuery,
  type SurrealQLAnalyzerClient,
} from "@surrealdb/analyzer-client";
import { getQueryClient } from "@surrealdb/analyzer-query";
import { useClient } from "./context.js";

export interface MutationOptions<R> {
  client?: SurrealQLAnalyzerClient;
  /**
   * Queries this write makes stale. Deliberately untyped as {@link AnyQuery}:
   * an invalidation target has no reason to agree with the mutation's own
   * result or parameter types.
   */
  invalidates?: readonly AnyQuery[];
  onSuccess?(data: Rows<R>): void;
  onError?(error: SurrealQLAnalyzerError): void;
}

export interface MutationHandle<R, P extends Record<string, unknown>> {
  /** Fire and forget; errors land on `.error`. */
  mutate(...args: ParamsArg<P>): void;
  /** Await the result; errors throw. */
  mutateAsync(...args: ParamsArg<P>): Promise<Rows<R>>;
  readonly data: Rows<R> | undefined;
  readonly error: SurrealQLAnalyzerError | undefined;
  readonly pending: boolean;
  reset(): void;
}

export function createMutation<R, P extends Record<string, unknown>>(
  query: SurqlQuery<R, P>,
  options: MutationOptions<R> = {},
): MutationHandle<R, P> {
  const client = options.client ?? useClient();

  let data = $state.raw<Rows<R> | undefined>(undefined);
  let error = $state.raw<SurrealQLAnalyzerError | undefined>(undefined);
  let pending = $state(false);

  const run = async (params?: Record<string, unknown>): Promise<Rows<R>> => {
    pending = true;
    error = undefined;
    try {
      const result = (await getQueryClient(client).mutate(
        query as unknown as SurqlQuery<R, Record<string, unknown>>,
        params as never,
        { invalidates: options.invalidates },
      )) as Rows<R>;
      data = result;
      options.onSuccess?.(result);
      return result;
    } catch (cause) {
      const wrapped = SurrealQLAnalyzerError.from(cause, { query: query.text, params });
      error = wrapped;
      options.onError?.(wrapped);
      throw wrapped;
    } finally {
      pending = false;
    }
  };

  return {
    mutate: (...args) => {
      void run(args[0] as Record<string, unknown> | undefined).catch(() => {
        // Already surfaced on `.error`; swallow so `mutate` never produces an
        // unhandled rejection. Use `mutateAsync` to handle it yourself.
      });
    },
    mutateAsync: (...args) => run(args[0] as Record<string, unknown> | undefined),
    get data() {
      return data;
    },
    get error() {
      return error;
    },
    get pending() {
      return pending;
    },
    reset() {
      data = undefined;
      error = undefined;
      pending = false;
    },
  };
}
