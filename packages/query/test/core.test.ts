import { describe, expect, it, vi } from "vitest";
import type { SurrealGuardClient } from "@surrealguard/client";
import type { LiveMessage } from "surrealdb";
import { QueryClient } from "../src/index.js";

const isLive = (sql: string) => /^\s*live\b/i.test(sql);

/** A fake client: canned rows, a capturable live handler, kill tracking. */
function makeClient() {
  let handler: ((m: LiveMessage) => void) | null = null;
  const killed: string[] = [];
  const liveOf = vi.fn(async () => ({
    id: "live-1",
    subscribe(h: (m: LiveMessage) => void) {
      handler = h;
      return () => {
        handler = null;
      };
    },
    async kill() {
      killed.push("live-1");
    },
  }));
  const client = {
    // The SDK's `query` resolves to the per-statement tuple; a LIVE SELECT
    // resolves to its live-query id.
    async query(sql: string) {
      return isLive(sql) ? ["live-1"] : [[{ id: "user:1", name: "ada" }]];
    },
    liveOf,
  } as unknown as SurrealGuardClient;
  return { client, killed, liveOf, emit: (m: LiveMessage) => handler?.(m) };
}

/** Build a live change notification in the SDK's `LiveMessage` shape. */
const change = (action: "CREATE" | "UPDATE" | "DELETE", id: string, value: Record<string, unknown> = {}) =>
  ({ queryId: "live-1", action, recordId: id, value }) as unknown as LiveMessage;

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const ids = (rows: Array<Record<string, unknown>>) => rows.map((r) => String(r.id));

describe("QueryClient", () => {
  it("resolves a one-shot query and reports success", async () => {
    const { client } = makeClient();
    const qc = new QueryClient(client);
    const obs = qc.observe("SELECT * FROM user");
    const unsub = obs.subscribe(() => {});
    await flush();
    expect(obs.get().status).toBe("success");
    expect(obs.get().data).toEqual([{ id: "user:1", name: "ada" }]);
    unsub();
  });

  it("reconciles live notifications by record id", async () => {
    const { client, emit } = makeClient();
    const qc = new QueryClient(client);
    const obs = qc.observe("LIVE SELECT * FROM user");
    const unsub = obs.subscribe(() => {});
    await flush();
    expect(obs.get().status).toBe("success");

    emit(change("CREATE", "user:1", { name: "ada" }));
    expect(ids(obs.get().data)).toEqual(["user:1"]);

    emit(change("CREATE", "user:2", { name: "lin" }));
    expect(ids(obs.get().data)).toEqual(["user:1", "user:2"]);

    emit(change("UPDATE", "user:2", { name: "linn" }));
    expect(obs.get().data.find((r) => String(r.id) === "user:2")?.name).toBe("linn");

    emit(change("DELETE", "user:1"));
    expect(ids(obs.get().data)).toEqual(["user:2"]);
    unsub();
  });

  it("shares one live subscription and kills it on the last unsubscribe", async () => {
    const { client, killed, liveOf } = makeClient();
    const qc = new QueryClient(client);
    const obs = qc.observe("LIVE SELECT * FROM user");
    const a = obs.subscribe(() => {});
    const b = obs.subscribe(() => {});
    await flush();
    expect(liveOf).toHaveBeenCalledTimes(1);

    a();
    expect(killed).toEqual([]);
    b();
    expect(killed).toEqual(["live-1"]);
  });

  it("carries data across the SSR boundary via dehydrate/hydrate", async () => {
    const { client } = makeClient();
    const server = new QueryClient(client);
    await server.fetch("SELECT * FROM user");
    const snapshot = server.dehydrate();
    expect(snapshot["SELECT * FROM user"]).toEqual([{ id: "user:1", name: "ada" }]);

    const consumer = new QueryClient(client);
    consumer.hydrate(snapshot);
    const obs = consumer.observe("SELECT * FROM user");
    expect(obs.get().status).toBe("success");
    expect(obs.get().data).toEqual([{ id: "user:1", name: "ada" }]);
  });
});
