import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import {
  defineLive,
  preload,
  type Json,
  type Preloaded,
  type RecordId as RecordIdType,
  type SurrealQLAnalyzerClient,
} from "@surrealdb/analyzer-client";
import { RecordId, type LiveMessage } from "surrealdb";
import { SurrealQLAnalyzerProvider, useLive } from "../src/index.js";

declare module "@surrealdb/analyzer-client" {
  interface SurqlRegistry {
    "SELECT * FROM user": {
      result: [Array<{ id: RecordIdType<"user">; name: string }>];
      params: Record<string, never>;
    };
  }
}

// The one place the query text lives — exactly as an app would do it.
const liveUsers = defineLive("SELECT * FROM user");
type Row = Json<{ id: RecordIdType<"user">; name: string }>;

// Vitest is not running with `globals: true`, so RTL's auto-cleanup is not
// installed; unmount between tests explicitly.
afterEach(cleanup);

const isLive = (sql: string) => /^\s*live\b/i.test(sql);

function makeClient(rows: Array<Record<string, unknown>> = []) {
  let handler: ((message: LiveMessage) => void) | null = null;
  const liveOf = vi.fn(async () => ({
    id: "live-1",
    subscribe(next: (message: LiveMessage) => void) {
      handler = next;
      return () => {
        handler = null;
      };
    },
    async kill() {},
  }));
  const query = vi.fn(async (sql: string) => (isLive(sql) ? ["live-1"] : [rows]));
  const client = {
    query,
    surreal: { query, liveOf },
    onInvalidate: () => () => {},
    runLiveOnce: async () => rows,
  } as unknown as SurrealQLAnalyzerClient;
  return { client, query, emit: (m: LiveMessage) => handler?.(m) };
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

const flush = () => act(async () => {
  await new Promise((resolve) => setTimeout(resolve, 0));
});

function Users({ preloaded }: { preloaded?: Preloaded<Row[]> }) {
  // The reference (or the preloaded payload) is passed DIRECTLY — no thunk.
  // Its `key` is stable, so React's memo dependency is `[client, source.key]`.
  const users = useLive(preloaded ?? liveUsers);
  if (users.error) return <p data-testid="error">{users.error.message}</p>;
  return (
    <>
      <p data-testid="status">{users.status}</p>
      <ul>
        {users.data.map((user) => (
          <li key={user.id}>{user.name}</li>
        ))}
      </ul>
    </>
  );
}

const withClient = (client: SurrealQLAnalyzerClient, node: React.ReactNode) => (
  <SurrealQLAnalyzerProvider client={client}>{node}</SurrealQLAnalyzerProvider>
);

describe("useLive", () => {
  it("subscribes through context and reconciles a live CREATE", async () => {
    const { client, emit } = makeClient([{ id: new RecordId("user", 1), name: "ada" }]);

    render(withClient(client, <Users />));
    await flush();

    // Seeded from the underlying SELECT — a LIVE SELECT never replays records.
    expect(screen.getByText("ada")).toBeTruthy();
    expect(screen.getByTestId("status").textContent).toBe("success");

    await act(async () => {
      emit(change("CREATE", 2, { name: "lin" }));
    });

    expect(screen.getByText("ada")).toBeTruthy();
    expect(screen.getByText("lin")).toBeTruthy();
  });

  it("hydrates from a Preloaded payload without naming the query again", async () => {
    const { client, query } = makeClient([{ id: new RecordId("user", 1), name: "ada" }]);
    const preloaded = await preload(client, liveUsers);

    // The payload carries the key the server used, so an RSC and its client
    // component cannot drift apart — the failure 0.4 had no defence against.
    expect(preloaded.key).toBe(liveUsers.key);
    // …and it is plain values, which is what an RSC boundary accepts at all.
    expect(preloaded.data).toEqual([{ id: "user:1", name: "ada" }]);

    query.mockClear();
    render(withClient(client, <Users preloaded={preloaded} />));

    // Rendered from the seed on the very first paint, with no refetch.
    expect(screen.getByText("ada")).toBeTruthy();
    expect(query).not.toHaveBeenCalledWith("SELECT * FROM user");
  });

  it("surfaces an error with its message", async () => {
    const client = {
      query: async () => {
        throw new Error("connection refused");
      },
      surreal: {},
      onInvalidate: () => () => {},
    } as unknown as SurrealQLAnalyzerClient;

    render(withClient(client, <Users />));
    await flush();

    expect(screen.getByTestId("error").textContent).toContain("connection refused");
  });

  it("throws a useful message with no provider", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(() => render(<Users />)).toThrow(/No client in context/);
    spy.mockRestore();
  });
});
