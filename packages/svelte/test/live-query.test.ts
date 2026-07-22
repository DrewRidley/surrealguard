import { describe, expect, it } from "vitest";
import {
  SurrealGuardClient,
  type Connection,
  type LiveNotification,
} from "@surrealguard/client";
import { QueryClient, type QueryState } from "@surrealguard/query";
import { liveQuery } from "../src/index.js";

/** A fake connection: canned rows plus a capturable live callback. */
function makeConn() {
  let liveCb: ((n: LiveNotification) => void) | null = null;
  const conn: Connection = {
    async query() {
      return [{ id: "user:1", name: "ada" }];
    },
    async live(_sql, cb) {
      liveCb = cb;
      return "live-1";
    },
    async kill() {},
  };
  return { conn, emit: (n: LiveNotification) => liveCb?.(n) };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const ids = (rows: Array<Record<string, unknown>>) => rows.map((r) => r.id);

describe("liveQuery", () => {
  it("seeds from initial and updates the store on a live notification", async () => {
    const { conn, emit } = makeConn();
    const qc = new QueryClient(new SurrealGuardClient(conn));

    const store = liveQuery(qc, "LIVE SELECT * FROM user", {
      initial: [{ id: "user:1", name: "ada" }],
    });

    let latest: QueryState<Record<string, unknown>> | undefined;
    const unsub = store.subscribe((s) => {
      latest = s;
    });

    // The store's first value is the seed data — a gap-free first render.
    expect(latest?.data).toEqual([{ id: "user:1", name: "ada" }]);

    // Let the observable start: fetch the seed rows and open the live sub.
    await flush();
    expect(ids(latest!.data)).toEqual(["user:1"]);

    // A live CREATE reconciles into the store and notifies the subscriber.
    emit({ action: "CREATE", result: { id: "user:2", name: "lin" } });
    expect(ids(latest!.data)).toEqual(["user:1", "user:2"]);

    unsub();
  });
});
