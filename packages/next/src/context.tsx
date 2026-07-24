"use client";

/**
 * Client context. Wrap the app (or a subtree) in {@link SurrealGuardProvider}
 * once; every {@link useLiveQuery} below it resolves that client, so components
 * never thread `db` through props.
 *
 * ```tsx
 * // app/providers.tsx
 * "use client";
 * import { SurrealGuardProvider } from "@surrealguard/next";
 * import { db } from "@/lib/db";
 * export function Providers({ children }: { children: React.ReactNode }) {
 *   return <SurrealGuardProvider client={db}>{children}</SurrealGuardProvider>;
 * }
 * ```
 */

import { createContext, useContext, type ReactNode } from "react";
import type { SurrealGuardClient } from "@surrealguard/client";

const ClientContext = createContext<SurrealGuardClient | null>(null);

export interface SurrealGuardProviderProps {
  client: SurrealGuardClient;
  children: ReactNode;
}

/** Provide the typed client to descendant components. */
export function SurrealGuardProvider({ client, children }: SurrealGuardProviderProps) {
  return <ClientContext.Provider value={client}>{children}</ClientContext.Provider>;
}

/**
 * Read the client from context. An explicit `override` wins (e.g. tests,
 * multiple connections). Throws with a clear message if neither is present —
 * failing fast beats a confusing "cannot read property of undefined".
 */
export function useClient(override?: SurrealGuardClient): SurrealGuardClient {
  const ctx = useContext(ClientContext);
  const client = override ?? ctx;
  if (!client) {
    throw new Error(
      "[@surrealguard/next] No client in context. Wrap your app in " +
        "<SurrealGuardProvider client={db}>, or pass { client } to useLiveQuery.",
    );
  }
  return client;
}
