import { describe, expect, it, vi } from "vitest";
import { SurrealGuardClient, type Connection, type LiveNotification } from "@surrealguard/client";
import { QueryClient } from "../src/index.js";

/** A fake connection: canned rows, a capturable live callback, kill tracking. */
function makeConn() {
  let liveCb: ((n: LiveNotification) => void) | null = null;
  const killed: string[] = [];
  const conn: Connection = {
    async query() {
      return [{ id: "user:1", name: "ada" }];
    },
    async live(_sql, cb) {
      liveCb = cb;
      return "live-1";
    },
    async kill(id) {
      killed.push(id);
    },
  };
  return {
    conn,
    killed,
    emit: (n: LiveNotification) => liveCb?.(n),
  };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const ids = (rows: Array<Record<string, unknown>>) => rows.map((r) => r.id);

describe("QueryClient", () => {
  it("resolves a one-shot query and reports success", async () => {
    const { conn } = makeConn();
    const qc = new QueryClient(new SurrealGuardClient(conn));
    const obs = qc.observe("SELECT * FROM user");
    const unsub = obs.subscribe(() => {});
    await flush();
    expect(obs.get().status).toBe("success");
    expect(obs.get().data).toEqual([{ id: "user:1", name: "ada" }]);
    unsub();
  });

  it("reconciles live notifications by record id", async () => {
    const { conn, emit } = makeConn();
    const qc = new QueryClient(new SurrealGuardClient(conn));
    const obs = qc.observe("LIVE SELECT * FROM user");
    const unsub = obs.subscribe(() => {});
    await flush();
    expect(ids(obs.get().data)).toEqual(["user:1"]);

    emit({ action: "CREATE", result: { id: "user:2", name: "lin" } });
    expect(ids(obs.get().data)).toEqual(["user:1", "user:2"]);

    emit({ action: "UPDATE", result: { id: "user:2", name: "linn" } });
    expect(obs.get().data.find((r) => r.id === "user:2")?.name).toBe("linn");

    emit({ action: "DELETE", result: { id: "user:1" } });
    expect(ids(obs.get().data)).toEqual(["user:2"]);
    unsub();
  });

  it("shares one live subscription and kills it on the last unsubscribe", async () => {
    const { conn, killed } = makeConn();
    const liveSpy = vi.spyOn(conn, "live");
    const qc = new QueryClient(new SurrealGuardClient(conn));
    const obs = qc.observe("LIVE SELECT * FROM user");
    const a = obs.subscribe(() => {});
    const b = obs.subscribe(() => {});
    await flush();
    expect(liveSpy).toHaveBeenCalledTimes(1);

    a();
    expect(killed).toEqual([]);
    b();
    expect(killed).toEqual(["live-1"]);
  });

  it("carries data across the SSR boundary via dehydrate/hydrate", async () => {
    const { conn } = makeConn();
    const server = new QueryClient(new SurrealGuardClient(conn));
    await server.fetch("SELECT * FROM user");
    const snapshot = server.dehydrate();
    expect(snapshot["SELECT * FROM user"]).toEqual([{ id: "user:1", name: "ada" }]);

    const client = new QueryClient(new SurrealGuardClient(conn));
    client.hydrate(snapshot);
    const obs = client.observe("SELECT * FROM user");
    expect(obs.get().status).toBe("success");
    expect(obs.get().data).toEqual([{ id: "user:1", name: "ada" }]);
  });
});
