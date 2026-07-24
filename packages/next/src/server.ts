/**
 * Server-side helpers (`@surrealguard/next/server`). No `"use client"` — safe to
 * call from a Server Component, `getServerSideProps`, or a route handler.
 *
 * A `LIVE SELECT` cannot resolve rows in one shot, so {@link queryServer} runs
 * its underlying `SELECT` once and returns the typed rows. Pass them to
 * {@link useLiveQuery}'s `initialData` for a gap-free first paint that then
 * upgrades to live on the client.
 *
 * ```tsx
 * // app/users/page.tsx  (Server Component)
 * import { queryServer } from "@surrealguard/next/server";
 * import { db } from "@/lib/db";
 * import { UsersList } from "./users-list"; // "use client", uses useLiveQuery
 * export default async function Page() {
 *   const users = await queryServer(db, db.live(`SELECT * FROM user`));
 *   return <UsersList initialData={users} />;
 * }
 * ```
 *
 * For whole-cache transport, {@link dehydrate} on the server and {@link hydrate}
 * on the client (e.g. inside a `<HydrationBoundary>`-style component).
 */

import type { LiveDescriptor, SurrealGuardClient } from "@surrealguard/client";
import { getQueryClient, type DehydratedState } from "@surrealguard/query";

/**
 * Run a live descriptor's underlying `SELECT` once and return the typed rows,
 * caching them under the live key so a later {@link useLiveQuery} of the same
 * query renders without a refetch, then upgrades to live.
 */
export function queryServer<Row>(
  client: SurrealGuardClient,
  descriptor: LiveDescriptor<Row>,
  params?: Record<string, unknown>,
): Promise<Row[]> {
  return getQueryClient(client).prime(descriptor, params);
}

/** Snapshot a client's cached results for transport to the browser (SSR). */
export function dehydrate(client: SurrealGuardClient): DehydratedState {
  return getQueryClient(client).dehydrate();
}

/** Seed a client's cache from a server snapshot so the browser avoids a refetch. */
export function hydrate(client: SurrealGuardClient, state: DehydratedState): void {
  getQueryClient(client).hydrate(state);
}
