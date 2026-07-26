/**
 * Server-side helpers (`@surrealguard/next/server`). No `"use client"` — safe
 * to call from a Server Component, a route handler, or `getServerSideProps`.
 *
 * ## Use a per-request client
 *
 * A module-level `export const db = createClient(...)` is a real hazard on the
 * server: Next imports that module into the server runtime, so **one connection
 * — one auth session, one cache — is shared by every concurrent request and
 * every user**. Any `signin()` mutates global state for everyone. Scope it with
 * React's `cache()`:
 *
 * ```ts
 * // lib/db.server.ts
 * import { cache } from "react";
 * import { createClient } from "@/surrealguard.generated";
 *
 * export const getDb = cache(() =>
 *   createClient({
 *     url: process.env.SURREAL_URL!,
 *     namespace: "app",
 *     database: "app",
 *   }),
 * );
 * ```
 *
 * ## Read data in the Server Component
 *
 * The default way to read data in the App Router is to await it in an RSC,
 * shipping zero client JS. That needs nothing from this package:
 *
 * ```tsx
 * // app/people/page.tsx
 * const people = await getDb().runJson(allPeople);
 * return <StaticRoster rows={people} />;
 * ```
 *
 * Use `runJson`, not `run`: an RSC → client-component boundary accepts only
 * plain values, and it has no transport hook. A `RecordId` instance crossing it
 * throws "Only plain objects can be passed to Client Components".
 *
 * ## Seed a live client component
 *
 * ```tsx
 * import { preload } from "@surrealguard/next/server";
 *
 * export default async function Page() {
 *   const preloaded = await preload(getDb(), livePeople);
 *   return <PeopleList preloaded={preloaded} />;
 * }
 * ```
 *
 * The payload carries its own key, so the client subscribes to exactly the
 * query the server ran and the text is written once, in `lib/queries.ts`.
 *
 * ## Stream a slow query
 *
 * Pass an *un-awaited* promise and `use()` it in a client component:
 *
 * ```tsx
 * // page.tsx (server)
 * const rows = getDb().runJson(slowReport);          // not awaited
 * return <Suspense fallback={<Skeleton />}><Report rows={rows} /></Suspense>;
 *
 * // report.tsx (client) -> const data = use(rows);
 * ```
 */

import { preload, type SurrealGuardClient } from "@surrealguard/client";
import { getQueryClient, type DehydratedState } from "@surrealguard/query";

export { preload };
export type { Preloaded, Json } from "@surrealguard/client";

/** Snapshot a client's cached results for transport to the browser. */
export function dehydrate(client: SurrealGuardClient): DehydratedState {
  return getQueryClient(client).dehydrate();
}

/** Seed a client's cache from a server snapshot so the browser avoids a refetch. */
export function hydrate(client: SurrealGuardClient, state: DehydratedState): void {
  getQueryClient(client).hydrate(state);
}
