import { describe, expect, it, vi } from "vitest";
import { flushSync } from "svelte";
import { render, screen } from "@testing-library/svelte";
import { preload, type SurrealQLAnalyzerClient } from "@surrealdb/analyzer-client";
import { RecordId, type LiveMessage } from "surrealdb";
import Users from "./Users.svelte";
import { liveUsers } from "./queries.js";

// The context key `setClient` / `useClient` use — pass it via render's
// `context` to exercise the real context path (no explicit `client` override).
const CLIENT_KEY = Symbol.for("@surrealdb/analyzer-svelte:client");
const isLive = (sql: string) => /^\s*live\b/i.test(sql);

/** A fake client with a canned `query`, a capturable live handler, kill tracking. */
function makeClient(rows: Array<Record<string, unknown>> = []) {
  let handler: ((message: LiveMessage) => void) | null = null;
  const killed: string[] = [];
  const subscribed: string[] = [];
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
    if (isLive(sql)) {
      subscribed.push(sql);
      return ["live-1"];
    }
    return [rows];
  });
  const client = {
    query,
    surreal: { query, liveOf },
    onInvalidate: () => () => {},
    runLiveOnce: async () => rows,
  } as unknown as SurrealQLAnalyzerClient;
  return { client, killed, subscribed, query, emit: (m: LiveMessage) => handler?.(m) };
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

describe("createLive", () => {
  it("subscribes from a component and reconciles a live CREATE", async () => {
    const { client, emit } = makeClient([{ id: new RecordId("user", 1), name: "ada" }]);

    render(Users, { context: new Map([[CLIENT_KEY, client]]) });
    await flush();

    // Seeded from the underlying SELECT — a LIVE SELECT never replays records.
    expect(screen.getByText("ada")).toBeTruthy();
    expect(screen.getByTestId("status").textContent).toBe("success");

    emit(change("CREATE", 2, { name: "lin" }));
    flushSync();

    expect(screen.getByText("ada")).toBeTruthy();
    expect(screen.getByText("lin")).toBeTruthy();
  });

  it("re-subscribes when the source thunk's dependencies change", async () => {
    // The regression this whole redesign exists for: in 0.4, `{ params }` was
    // read once at construction, so navigating to another team did nothing.
    const { client, subscribed, killed } = makeClient([]);

    const { rerender } = render(Users, {
      props: { team: "red" },
      context: new Map([[CLIENT_KEY, client]]),
    });
    await flush();
    expect(subscribed).toHaveLength(1);

    await rerender({ team: "blue" });
    await flush();

    // A second, different subscription opened…
    expect(subscribed).toHaveLength(2);
    // …and the first was killed rather than leaked.
    expect(killed).toEqual(["live-1"]);
  });

  it("hydrates from a Preloaded payload without naming the query again", async () => {
    const { client, query } = makeClient([{ id: new RecordId("user", 1), name: "ada" }]);
    const preloaded = await preload(client, liveUsers);

    // The payload carries the key, so the component subscribes to exactly the
    // query the server ran — no second reference to the text, nothing to drift.
    expect(preloaded.key).toBe(liveUsers.key);
    expect(preloaded.data).toEqual([{ id: "user:1", name: "ada" }]);

    query.mockClear();
    render(Users, {
      props: { preloaded },
      context: new Map([[CLIENT_KEY, client]]),
    });

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

    render(Users, { context: new Map([[CLIENT_KEY, client]]) });
    await flush();
    flushSync();

    expect(screen.getByTestId("status").textContent).toBe("error");
    expect(screen.getByTestId("error").textContent).toContain("connection refused");
  });
});
