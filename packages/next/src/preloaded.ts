/**
 * Rebuilding a query reference from a {@link Preloaded} payload.
 *
 * This is what lets a client component subscribe to exactly the query a Server
 * Component ran, **without naming it again** — the structural flaw the redesign
 * exists to fix. In 0.4 the RSC and the client component each spelled the query
 * text out; change one and the key stopped matching, so the seed was silently
 * discarded and the page refetched, with no error and no type failure.
 *
 * Shared by the client hooks and kept out of the `"use client"` entry so the
 * server entry can import it too.
 */

import type { Bound, Preloaded, SurqlLive } from "@surrealguard/client";

export function isPreloaded(value: unknown): value is Preloaded<unknown> {
  return (
    typeof value === "object" &&
    value !== null &&
    "data" in value &&
    "key" in value &&
    !("with" in value)
  );
}

export function fromPreloaded(payload: Preloaded<unknown>): SurqlLive<unknown, Bound> {
  return {
    text: payload.text,
    liveText: payload.liveText ?? `LIVE ${payload.text}`,
    params: payload.params,
    key: payload.key as SurqlLive<unknown, Bound>["key"],
    isLive: payload.isLive,
    with: () => fromPreloaded(payload),
  } as SurqlLive<unknown, Bound>;
}
