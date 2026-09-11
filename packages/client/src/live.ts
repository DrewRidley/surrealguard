/**
 * Live-subscription plumbing, shared by `db.watch` and `@surrealdb/analyzer-query`.
 *
 * A `LIVE SELECT` resolves to a live-query id; subscribing to that id's change
 * stream yields notifications which must be reconciled into an array by record
 * `id` (CREATE appends, UPDATE replaces, DELETE removes). Both the vanilla
 * `db.watch` and the refcounted reactive core need exactly that, so it lives
 * here once rather than in each.
 */

import type { LiveMessage, LiveSubscription, Surreal, Uuid } from "surrealdb";

/** A row as the reconciler sees it: anything with an `id` to key on. */
export type ReconcilableRow = Record<string, unknown> & { id?: unknown };

/**
 * Apply one change notification to a reconciled array, keyed by record `id`.
 * Returns a new array; the input is not mutated, so a reactive consumer sees a
 * fresh reference.
 *
 * `String(id)` drives the comparison so this works both on SDK values (where
 * `id` is a `RecordId`) and on their JSON projection (where it is the
 * `` `table:${string}` `` string). The reactive layer jsonifies and `db.watch`
 * does not, and one reconciler serves both.
 */
export function reconcile<Row extends ReconcilableRow>(
  rows: readonly Row[],
  message: LiveMessage,
  transform: (row: ReconcilableRow) => Row = (row) => row as Row,
): readonly Row[] {
  if (message.action === "KILLED") return rows;
  const id = String(message.recordId);
  const next = rows.slice();
  const at = next.findIndex((existing) => String(existing.id) === id);
  if (message.action === "DELETE") {
    if (at >= 0) next.splice(at, 1);
    return next;
  }
  const row = transform({ id: message.recordId, ...message.value });
  if (at >= 0) next[at] = row;
  else next.push(row);
  return next;
}

/**
 * Open a live subscription for a `LIVE SELECT …` text: resolve the live-query
 * id, then subscribe to its notification stream.
 */
export async function openLive(
  surreal: Surreal,
  liveText: string,
  params: Record<string, unknown> | undefined,
  onMessage: (message: LiveMessage) => void,
): Promise<LiveSubscription> {
  const [liveId] = await surreal.query(liveText, params);
  const subscription = await surreal.liveOf(liveId as Uuid);
  subscription.subscribe(onMessage);
  return subscription;
}
