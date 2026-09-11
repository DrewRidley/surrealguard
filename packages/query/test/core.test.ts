import { describe, expect, it, vi } from "vitest";
import { defineLive, defineQuery, type SurrealQLAnalyzerClient } from "@surrealdb/analyzer-client";
import { RecordId, type LiveMessage } from "surrealdb";
import { QueryClient } from "../src/index.js";

/**
 * A fake client: canned rows, a capturable live handler, kill tracking. The
 * shape mirrors what the real client exposes to the core — `query`, `surreal`
 * (for `liveOf`), `run`, and `onInvalidate`.
 */
function makeClient(
  rows: Array<Record<string, unknown>> = [{ id: new RecordId("user", 1), name: "ada" }],
) {
  let handler: ((message: LiveMessage) => void) | null = null;
  const killed: string[] = [];
  const queried: string[] = [];
  const liveOf = vi.fn(async () => ({
    id: "live-1",
    subscribe(next: (message: LiveMessage) => void) {
      handler = next;
      return () => {
        handler = null;
      };
    },
    async kill() {
      killed.push("live-1");
    },
  }));
  const query = vi.fn(async (sql: string) => {
    queried.push(sql);
    // The SDK returns one result per statement; a LIVE SELECT returns its id.
    return /^\s*live\b/i.test(sql) ? ["live-1"] : [rows];
  });
  const client = {
    query,
    surreal: { query, liveOf },
    onInvalidate: () => () => {},
  } as unknown as SurrealQLAnalyzerClient;
  return { client, killed, liveOf, queried, query, emit: (m: LiveMessage) => handler?.(m) };
}

const change = (
  action: "CREATE" | "UPDATE" | "DELETE",
  id: string | number,
  value: Record<string, unknown> = {},
) =>
  ({
    queryId: "live-1",
    action,
    recordId: new RecordId("user", id),
    value,
  }) as unknown as LiveMessage;

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const ids = (rows: unknown) => (rows as Array<{ id: unknown }>).map((row) => String(row.id));

// The registry is empty in this package, so queries are built through the
// `unchecked` factories; the typed path is proven in `@surrealdb/analyzer-client`.
const users = defineQuery.unchecked("SELECT * FROM user");
const liveUsers = defineLive.unchecked("SELECT * FROM user");

describe("QueryClient", () => {
  it("resolves a one-shot query and reports success", async () => {
    const { client } = makeClient();
    const observable = new QueryClient(client).observe(users);
    const unsubscribe = observable.subscribe(() => {});
    await flush();
    expect(observable.get().status).toBe("success");
    // Rows arrive jsonified: this layer is what crosses an SSR boundary, so a
    // RecordId is already the `user:1` string it will be on the other side.
    expect(observable.get().data).toEqual([{ id: "user:1", name: "ada" }]);
    unsubscribe();
  });

  it("starts pending, with data undefined until it resolves", () => {
    const { client } = makeClient();
    const observable = new QueryClient(client).observe(users);
    const state = observable.get();
    expect(state.status).toBe("pending");
    expect(state.data).toBeUndefined();
  });

  it("observes a scalar result, not just rows", async () => {
    // A one-statement query whose result is a number: 0.4 could not observe
    // this at all, because `data` was hard-coded to `Row[]`.
    const client = {
      query: async () => [42],
      surreal: {},
      onInvalidate: () => () => {},
    } as unknown as SurrealQLAnalyzerClient;
    const scalar = defineQuery.unchecked("RETURN 42");
    const observable = new QueryClient(client).observe(scalar);
    const unsubscribe = observable.subscribe(() => {});
    await flush();
    expect(observable.get().data).toBe(42);
    unsubscribe();
  });

  it("seeds a live query from its underlying SELECT, then reconciles by id", async () => {
    const { client, emit, queried } = makeClient([]);
    const observable = new QueryClient(client).observeLive(liveUsers);
    const unsubscribe = observable.subscribe(() => {});
    await flush();
    // A LIVE SELECT never replays existing records, so the bare SELECT runs
    // first. Without that seed the list would start empty and stay empty.
    expect(queried[0]).toBe("SELECT * FROM user");
    expect(queried[1]).toBe("LIVE SELECT * FROM user");
    expect(observable.get().status).toBe("success");

    emit(change("CREATE", 1, { name: "ada" }));
    expect(ids(observable.get().data)).toEqual(["user:1"]);

    emit(change("CREATE", 2, { name: "lin" }));
    expect(ids(observable.get().data)).toEqual(["user:1", "user:2"]);

    emit(change("UPDATE", 2, { name: "linn" }));
    const updated = observable.get().data as Array<{ id: string; name: string }>;
    expect(updated.find((row) => row.id === "user:2")?.name).toBe("linn");

    emit(change("DELETE", 1));
    expect(ids(observable.get().data)).toEqual(["user:2"]);
    unsubscribe();
  });

  it("shares one live subscription and kills it on the last unsubscribe", async () => {
    const { client, killed, liveOf } = makeClient();
    const observable = new QueryClient(client).observeLive(liveUsers);
    const first = observable.subscribe(() => {});
    const second = observable.subscribe(() => {});
    await flush();
    expect(liveOf).toHaveBeenCalledTimes(1);

    first();
    expect(killed).toEqual([]);
    second();
    expect(killed).toEqual(["live-1"]);
  });

  it("reports errors as SurrealQLAnalyzerError carrying the query", async () => {
    const client = {
      query: async () => {
        throw new Error("boom");
      },
      surreal: {},
      onInvalidate: () => () => {},
    } as unknown as SurrealQLAnalyzerClient;
    const observable = new QueryClient(client).observe(users);
    const unsubscribe = observable.subscribe(() => {});
    await flush();
    const state = observable.get();
    expect(state.status).toBe("error");
    expect(state.error?.query).toBe("SELECT * FROM user");
    expect(state.error?.cause).toBeInstanceOf(Error);
    unsubscribe();
  });

  it("carries data across the SSR boundary via dehydrate/hydrate", async () => {
    const { client } = makeClient();
    const server = new QueryClient(client);
    await server.fetch(users);
    const snapshot = server.dehydrate();
    expect(snapshot["SELECT * FROM user"]).toEqual([{ id: "user:1", name: "ada" }]);

    const consumer = new QueryClient(client);
    consumer.hydrate(snapshot);
    const observable = consumer.observe(users);
    expect(observable.get().status).toBe("success");
    expect(observable.get().data).toEqual([{ id: "user:1", name: "ada" }]);
  });

  it("types a cache read from the branded key alone", async () => {
    const { client } = makeClient();
    const queryClient = new QueryClient(client);
    await queryClient.fetch(users);
    // No annotation, no second reference to the query.
    expect(queryClient.getData(users.key)).toEqual([{ id: "user:1", name: "ada" }]);
  });

  it("setData writes optimistically", async () => {
    const { client } = makeClient();
    const queryClient = new QueryClient(client);
    await queryClient.fetch(users);
    queryClient.setData(users, (previous) => [
      ...((previous as Array<Record<string, unknown>> | undefined) ?? []),
      { id: "user:2", name: "lin" },
    ]);
    expect(ids(queryClient.getData(users.key))).toEqual(["user:1", "user:2"]);
  });

  it("refetches a subscribed entry on invalidate and drops an idle one", async () => {
    const { client, query } = makeClient();
    const queryClient = new QueryClient(client);
    const observable = queryClient.observe(users);
    const unsubscribe = observable.subscribe(() => {});
    await flush();
    expect(query).toHaveBeenCalledTimes(1);

    await queryClient.invalidate(users);
    expect(query).toHaveBeenCalledTimes(2);

    unsubscribe();
    await queryClient.invalidate(users);
    // No subscribers left, so the entry is dropped rather than refetched.
    expect(query).toHaveBeenCalledTimes(2);
    expect(queryClient.getData(users.key)).toBeUndefined();
  });

  it("invalidates every binding of a query, or exactly one", async () => {
    const { client } = makeClient();
    const queryClient = new QueryClient(client);
    const byTeam = defineQuery.unchecked("SELECT * FROM user WHERE team = $team");
    const red = byTeam.with({ team: "red" });
    const blue = byTeam.with({ team: "blue" });
    await queryClient.fetch(red);
    await queryClient.fetch(blue);

    // Bound: exactly that binding.
    await queryClient.invalidate(red);
    expect(queryClient.getData(red.key)).toBeUndefined();
    expect(queryClient.getData(blue.key)).toBeDefined();

    // Unbound: every binding, matched by key prefix.
    await queryClient.fetch(red);
    await queryClient.invalidate(byTeam);
    expect(queryClient.getData(red.key)).toBeUndefined();
    expect(queryClient.getData(blue.key)).toBeUndefined();
  });

  it("mutate runs the write and invalidates what it affects", async () => {
    const { client } = makeClient();
    const run = vi.fn(async () => [{ id: new RecordId("user", 3) }]);
    const queryClient = new QueryClient({ ...client, run } as unknown as SurrealQLAnalyzerClient);
    await queryClient.fetch(users);
    expect(queryClient.getData(users.key)).toBeDefined();

    const create = defineQuery.unchecked("CREATE user SET name = $name");
    await queryClient.mutate(create, { name: "ada" }, { invalidates: [users] });
    expect(run).toHaveBeenCalledOnce();
    expect(queryClient.getData(users.key)).toBeUndefined();
  });

  it("bounds the cache, dropping least-recently-used idle entries", async () => {
    const { client } = makeClient();
    const queryClient = new QueryClient(client, { maxEntries: 2 });
    const a = defineQuery.unchecked("SELECT 1");
    const b = defineQuery.unchecked("SELECT 2");
    const c = defineQuery.unchecked("SELECT 3");
    await queryClient.fetch(a);
    await queryClient.fetch(b);
    await queryClient.fetch(c);
    // 0.4 never evicted at all, so a long-lived SPA with parameterised queries
    // grew without bound.
    expect(queryClient.getData(a.key)).toBeUndefined();
    expect(queryClient.getData(c.key)).toBeDefined();
  });
});
