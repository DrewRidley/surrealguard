"use client";

/**
 * Client context. Wrap the app (or a subtree) in {@link SurrealQLAnalyzerProvider}
 * once; every hook below it resolves that client, so components never thread
 * `db` through props.
 *
 * ```tsx
 * // app/providers.tsx
 * "use client";
 * import { SurrealQLAnalyzerProvider } from "@surrealdb/analyzer-next";
 * import { db } from "@/lib/db";
 *
 * export function Providers({ children }: { children: React.ReactNode }) {
 *   return <SurrealQLAnalyzerProvider client={db}>{children}</SurrealQLAnalyzerProvider>;
 * }
 * ```
 *
 * Note this is the **browser** client. A Server Component must use a
 * per-request client instead — see `@surrealdb/analyzer-next/server`.
 */

import { createContext, useContext, type ReactNode } from "react";
import type { SurrealQLAnalyzerClient } from "@surrealdb/analyzer-client";

const ClientContext = createContext<SurrealQLAnalyzerClient | null>(null);

export interface SurrealQLAnalyzerProviderProps {
  client: SurrealQLAnalyzerClient;
  children: ReactNode;
}

/** Provide the typed client to descendant components. */
export function SurrealQLAnalyzerProvider({ client, children }: SurrealQLAnalyzerProviderProps) {
  return <ClientContext.Provider value={client}>{children}</ClientContext.Provider>;
}

/**
 * Read the client from context. An explicit `override` wins (tests, multiple
 * connections). Throws with a clear message if neither is present — failing
 * fast beats a confusing "cannot read property of undefined".
 */
export function useClient(override?: SurrealQLAnalyzerClient): SurrealQLAnalyzerClient {
  const ctx = useContext(ClientContext);
  const client = override ?? ctx;
  if (!client) {
    throw new Error(
      "[@surrealdb/analyzer-next] No client in context. Wrap your app in " +
        "<SurrealQLAnalyzerProvider client={db}>, or pass { client } to the hook.",
    );
  }
  return client;
}
