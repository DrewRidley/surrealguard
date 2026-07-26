/**
 * Next.js / React bindings for SurrealGuard (client entry — `"use client"`).
 *
 * - {@link SurrealGuardProvider} / {@link useClient} — provide the typed client.
 * - {@link useQuery} — a one-shot query with loading and error state.
 * - {@link useLive} — a live query; `data` stays reconciled.
 * - {@link useMutation} — a write plus the invalidation that follows it.
 *
 * Server helpers (`preload`, `dehydrate`, `hydrate`) live in the separate,
 * server-safe entry `@surrealguard/next/server`.
 */

export {
  SurrealGuardProvider,
  useClient,
  type SurrealGuardProviderProps,
} from "./context.js";
export {
  useLive,
  useMutation,
  useQuery,
  type LiveResult,
  type MutationResult,
  type QueryResult,
  type Skip,
  type UseMutationOptions,
  type UseQueryOptions,
} from "./hooks.js";
export { SurrealGuardError } from "@surrealguard/client";
export type { Json, Preloaded, RowOf, SurqlLive, SurqlQuery } from "@surrealguard/client";
