import { describe, expect, it, vi } from "vitest";
import { flushSync } from "svelte";
import { render, screen } from "@testing-library/svelte";
import type { SurrealGuardClient } from "@surrealguard/client";
import type { LiveMessage } from "surrealdb";
import Users from "./Users.svelte";

// The context key `setClient`/`getClient` use — pass it via render's `context`
// to exercise the real context path (no explicit `client` override).
const CLIENT_KEY = Symbol.for("@surrealguard/svelte:client");
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

describe("liveQuery (Svelte 5 runes)", () => {
  it("seeds initial, reads users.data directly, and updates on a live CREATE", async () => {
    const { client, emit } = makeClient();

    render(Users, {
      props: { initial: [{ id: "user:1", name: "ada" }] },
      context: new Map([[CLIENT_KEY, client]]),
    });

    // Seeded initial is on the first render — a gap-free start, read directly.
    expect(screen.getByText("ada")).toBeTruthy();
    expect(screen.queryByText("lin")).toBeNull();

    // Let the $effect subscribe and the live query register.
    await flush();

    // A live CREATE reconciles into the runes-backed state and re-renders.
    emit(change("CREATE", "user:2", { name: "lin" }));
    flushSync();

    expect(screen.getByText("ada")).toBeTruthy();
    expect(screen.getByText("lin")).toBeTruthy();
  });
});
