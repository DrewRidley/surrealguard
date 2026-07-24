import { describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import type { SurrealGuardClient } from "@surrealguard/client";
import type { LiveMessage } from "surrealdb";
import { SurrealGuardProvider, useLiveQuery } from "../src/index.js";

const isLive = (sql: string) => /^\s*live\b/i.test(sql);

/** A fake client with `.live()`, a canned `.query()`, and a capturable live handler. */
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
    live(sql: string) {
      return { sql: isLive(sql) ? sql : `LIVE ${sql}` };
    },
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

/** Resolves its client from context (via useClient), no explicit override. */
function Users() {
  const { data } = useLiveQuery((db) => db.live(`SELECT * FROM user`), {
    initialData: [{ id: "user:1", name: "ada" }] as Array<Record<string, unknown>>,
  });
  return (
    <ul>
      {data.map((u) => (
        <li key={String(u.id)}>{String(u.name)}</li>
      ))}
    </ul>
  );
}

describe("useLiveQuery", () => {
  it("renders seeded data from context client and re-renders on a live CREATE", async () => {
    const { client, emit } = makeClient();

    render(
      <SurrealGuardProvider client={client}>
        <Users />
      </SurrealGuardProvider>,
    );

    // Seeded initialData is visible on the first render, no hydration gap.
    expect(screen.getByText("ada")).toBeTruthy();
    expect(screen.queryByText("lin")).toBeNull();

    // Let the observable start: subscribe to the live query.
    await act(async () => {
      await flush();
    });

    // A live CREATE pushes through the core and re-renders the hook.
    await act(async () => {
      emit(change("CREATE", "user:2", { name: "lin" }));
    });

    expect(screen.getByText("ada")).toBeTruthy();
    expect(screen.getByText("lin")).toBeTruthy();
  });
});
