import { describe, expect, it } from "vitest";
import { act, render, screen } from "@testing-library/react";
import {
  SurrealGuardClient,
  type Connection,
  type LiveNotification,
} from "@surrealguard/client";
import { QueryClient } from "@surrealguard/query";
import { useLiveQuery } from "../src/index.js";

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
    const { conn, emit } = makeConn();
    const qc = new QueryClient(new SurrealGuardClient(conn));

    render(<Users client={qc} />);

    // Seeded initialData is visible on the first render, no hydration gap.
    expect(screen.getByText("ada")).toBeTruthy();
    expect(screen.queryByText("lin")).toBeNull();

    // Let the observable start: fetch the seed rows and open the live sub.
    await act(async () => {
      await flush();
    });

    // A live CREATE pushes through the QueryClient and re-renders the hook.
    await act(async () => {
      emit({ action: "CREATE", result: { id: "user:2", name: "lin" } });
    });

    expect(screen.getByText("ada")).toBeTruthy();
    expect(screen.getByText("lin")).toBeTruthy();
  });
});
