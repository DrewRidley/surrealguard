import { describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import type { SurrealGuardClient } from "@surrealguard/client";
import type { LiveMessage } from "surrealdb";
import { QueryClient } from "@surrealguard/query";
import { useLiveQuery } from "../src/index.js";

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

function Users({ client }: { client: QueryClient }) {
  const state = useLiveQuery(client, "LIVE SELECT * FROM user", {
    initialData: [{ id: "user:1", name: "ada" }],
  });
  return (
    <ul>
      {state.data.map((u) => (
        <li key={String(u.id)}>{String(u.name)}</li>
      ))}
    </ul>
  );
}

describe("useLiveQuery", () => {
  it("renders seeded data and re-renders on a live CREATE notification", async () => {
    const { client, emit } = makeClient();
    const qc = new QueryClient(client);

    render(<Users client={qc} />);

    // Seeded initialData is visible on the first render, no hydration gap.
    expect(screen.getByText("ada")).toBeTruthy();
    expect(screen.queryByText("lin")).toBeNull();

    // Let the observable start: subscribe to the live query.
    await act(async () => {
      await flush();
    });

    // A live CREATE pushes through the QueryClient and re-renders the hook.
    await act(async () => {
      emit(change("CREATE", "user:2", { name: "lin" }));
    });

    expect(screen.getByText("ada")).toBeTruthy();
    expect(screen.getByText("lin")).toBeTruthy();
  });
});
