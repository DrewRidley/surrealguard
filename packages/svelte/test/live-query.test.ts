import { describe, expect, it, vi } from "vitest";
import type { SurrealGuardClient } from "@surrealguard/client";
import type { LiveMessage } from "surrealdb";
import { QueryClient, type QueryState } from "@surrealguard/query";
import { liveQuery } from "../src/index.js";

const isLive = (sql: string) => /^\s*live\b/i.test(sql);

/** A fake client: canned rows plus a capturable live handler. */
function makeClient() {
  let handler: ((m: LiveMessage) => void) | null = null;
  const liveOf = vi.fn(async () => ({
    id: "live-1",
    subscribe(h: (m: LiveMessage) => void) {
      handler = h;
      return () => {
        handler = null;
      };
    },
    async kill() {},
  }));
  const client = {
    async query(sql: string) {
      return isLive(sql) ? ["live-1"] : [[{ id: "user:1", name: "ada" }]];
    },
    liveOf,
  } as unknown as SurrealGuardClient;
  return { client, emit: (m: LiveMessage) => handler?.(m) };
}

const change = (action: "CREATE" | "UPDATE" | "DELETE", id: string, value: Record<string, unknown> = {}) =>
  ({ queryId: "live-1", action, recordId: id, value }) as unknown as LiveMessage;

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const ids = (rows: Array<Record<string, unknown>>) => rows.map((r) => String(r.id));

describe("liveQuery", () => {
  it("seeds from initial and updates the store on a live notification", async () => {
    const { client, emit } = makeClient();
    const qc = new QueryClient(client);

    const store = liveQuery(qc, "LIVE SELECT * FROM user", {
      initial: [{ id: "user:1", name: "ada" }],
    });

    let latest: QueryState<Record<string, unknown>> | undefined;
    const unsub = store.subscribe((s) => {
      latest = s;
    });

    // The store's first value is the seed data — a gap-free first render.
    expect(latest?.data).toEqual([{ id: "user:1", name: "ada" }]);

    // Let the observable start: subscribe to the live query.
    await flush();
    expect(ids(latest!.data)).toEqual(["user:1"]);

    // A live CREATE reconciles into the store and notifies the subscriber.
    emit(change("CREATE", "user:2", { name: "lin" }));
    expect(ids(latest!.data)).toEqual(["user:1", "user:2"]);

    unsub();
  });
});
