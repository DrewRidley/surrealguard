/**
 * `<Query>` / `<LiveQuery>` behaviour.
 *
 * The type-level half of these components — that a `children` snippet parameter
 * is inferred from `q` — is proved by `svelte-check` over `test-d/`. What is
 * left to prove at runtime is the part types cannot say: that a subscription is
 * opened once, shared, and **killed exactly when the component that wanted it
 * goes away**. Nesting a `<LiveQuery>` per row is the intended pattern, so a
 * row scrolled out of view has to stop costing a `LIVE SELECT`.
 */
import { describe, expect, it, vi } from "vitest";
import { flushSync } from "svelte";
import { render, screen } from "@testing-library/svelte";
import { preload, type SurrealGuardClient } from "@surrealguard/client";
import { RecordId, type LiveMessage } from "surrealdb";
import Pair from "./Pair.svelte";
import QueryStates from "./QueryStates.svelte";
import Roster from "./Roster.svelte";
import { liveUsers } from "./queries.js";

const CLIENT_KEY = Symbol.for("@surrealguard/svelte:client");
const isLive = (sql: string) => /^\s*live\b/i.test(sql);
const context = (client: SurrealGuardClient) => new Map([[CLIENT_KEY, client]]);
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

interface Row {
  id: RecordId<"user">;
  name: string;
}

/**
 * A fake client that gives every `LIVE SELECT` its **own** id and records the
 * kills, so "which subscription died" is answerable rather than "did any". The
 * one in `queries.test.ts` shares a single id, which cannot express the
 * question this file asks.
 */
function makeClient(rows: Row[] = []) {
  const handlers = new Map<string, (message: LiveMessage) => void>();
  const killed: string[] = [];
  /** live id -> the `LIVE SELECT` params it was opened with. */
  const opened = new Map<string, Record<string, unknown> | undefined>();
  let counter = 0;

  const query = vi.fn(async (sql: string, params?: Record<string, unknown>) => {
    if (isLive(sql)) {
      const id = `live-${++counter}`;
      opened.set(id, params);
      return [id];
    }
    // A `$name` filter is the only predicate these fixtures use.
    const name = params?.name;
    return [name === undefined ? rows : rows.filter((row) => row.name === name)];
  });

  const liveOf = vi.fn(async (id: string) => ({
    id,
    subscribe(next: (message: LiveMessage) => void) {
      handlers.set(id, next);
      return () => handlers.delete(id);
    },
    async kill() {
      killed.push(id);
      handlers.delete(id);
    },
  }));

  const client = {
    query,
    surreal: { query, liveOf },
    onInvalidate: () => () => {},
    runLiveOnce: async () => rows,
  } as unknown as SurrealGuardClient;

  /** The live id opened for a given `$name`, or undefined if none was. */
  const idFor = (name: string) =>
    [...opened].find(([, params]) => params?.name === name)?.[0];

  return { client, killed, opened, query, idFor, handlers };
}

const change = (name: string, id: number): LiveMessage =>
  ({
    queryId: "ignored",
    action: "CREATE",
    recordId: new RecordId("user", id),
    value: { name },
  }) as unknown as LiveMessage;

const users = (...names: string[]): Row[] =>
  names.map((name, index) => ({ id: new RecordId("user", index + 1), name }));

describe("<Query>", () => {
  it("renders `loading` first, then `children` with the typed rows", async () => {
    const { client } = makeClient(users("ada", "lin"));

    render(QueryStates, { context: context(client) });
    expect(screen.getByTestId("loading").textContent).toBe("loading");

    await flush();
    flushSync();

    expect(screen.queryByTestId("loading")).toBeNull();
    expect(screen.getByText("ada")).toBeTruthy();
    expect(screen.getByText("lin")).toBeTruthy();
  });

  it("renders the `error` snippet, and its retry refetches", async () => {
    let attempt = 0;
    const client = {
      query: vi.fn(async () => {
        attempt += 1;
        if (attempt === 1) throw new Error("connection refused");
        return [users("ada")];
      }),
      surreal: {},
      onInvalidate: () => () => {},
    } as unknown as SurrealGuardClient;

    render(QueryStates, { context: context(client) });
    await flush();
    flushSync();

    expect(screen.getByTestId("error").textContent).toContain("connection refused");
    // The error snippet claimed the failure, so the boundary never saw it.
    expect(screen.queryByTestId("boundary")).toBeNull();

    screen.getByTestId("retry").click();
    await flush();
    flushSync();

    expect(screen.queryByTestId("error")).toBeNull();
    expect(screen.getByText("ada")).toBeTruthy();
  });

  it("throws an unclaimed error instead of rendering nothing forever", async () => {
    // No `error` snippet. A component that quietly rendered blank would leave a
    // page that is broken and silent; throwing puts the failure where a
    // `<svelte:boundary>` (or SvelteKit's error page) can see it.
    const client = {
      query: vi.fn(async () => {
        throw new Error("connection refused");
      }),
      surreal: {},
      onInvalidate: () => () => {},
    } as unknown as SurrealGuardClient;

    render(QueryStates, { props: { handleError: false }, context: context(client) });
    await flush();
    flushSync();

    expect(screen.getByTestId("boundary").textContent).toContain("connection refused");
  });

  it("adopts a Preloaded payload on the first paint, with no refetch", async () => {
    const { client, query } = makeClient(users("ada"));
    const preloaded = await preload(client, liveUsers);
    query.mockClear();

    render(QueryStates, { props: { preloaded }, context: context(client) });

    // Synchronously, before any flush: no spinner, the seeded row is there.
    expect(screen.queryByTestId("loading")).toBeNull();
    expect(screen.getByText("ada")).toBeTruthy();
    expect(query).not.toHaveBeenCalled();
  });
});

describe("<LiveQuery>", () => {
  it("renders reconciled rows and never exposes the live id", async () => {
    const { client, idFor, handlers } = makeClient(users("ada"));

    render(Roster, { props: { visible: ["ada"] }, context: context(client) });
    await flush();
    flushSync();

    expect(screen.getByTestId("row-ada").textContent).toBe("ada");
    // The `Uuid` is a subscription handle; nothing in the DOM carries it.
    const id = idFor("ada")!;
    expect(document.body.textContent).not.toContain(id);

    handlers.get(id)!(change("ada-2", 9));
    flushSync();
    expect(screen.getByTestId("row-ada").textContent).toBe("ada,ada-2");
  });

  it("opens one subscription per row and KILLs the row that goes away", async () => {
    const { client, killed, idFor } = makeClient(users("ada", "lin"));

    const { rerender } = render(Roster, {
      props: { visible: ["ada", "lin"] },
      context: context(client),
    });
    await flush();
    flushSync();

    const ada = idFor("ada");
    const lin = idFor("lin");
    expect(ada).toBeDefined();
    expect(lin).toBeDefined();
    expect(ada).not.toBe(lin);
    expect(killed).toEqual([]);

    // "lin" scrolls out of view.
    await rerender({ visible: ["ada"] });
    await flush();

    expect(killed).toEqual([lin]);
    expect(screen.getByTestId("row-ada")).toBeTruthy();
    expect(screen.queryByTestId("row-lin")).toBeNull();
  });

  it("KILLs every row's subscription when the tree unmounts", async () => {
    const { client, killed, idFor } = makeClient(users("ada", "lin"));

    const { unmount } = render(Roster, {
      props: { visible: ["ada", "lin"] },
      context: context(client),
    });
    await flush();
    flushSync();

    unmount();
    await flush();

    expect(killed.sort()).toEqual([idFor("ada"), idFor("lin")].sort());
  });

  it("shares one subscription between components on the same query", async () => {
    const { client, killed, opened } = makeClient(users("ada"));

    const { rerender } = render(Pair, {
      props: { first: true, second: true },
      context: context(client),
    });
    await flush();
    flushSync();

    // Two components, one `LIVE SELECT`.
    expect(opened.size).toBe(1);
    expect(screen.getByTestId("first")).toBeTruthy();
    expect(screen.getByTestId("second")).toBeTruthy();

    // The first leaves: the other still reads it, so nothing is killed.
    await rerender({ first: false, second: true });
    await flush();
    expect(killed).toEqual([]);

    // The last one leaves: now it goes.
    await rerender({ first: false, second: false });
    await flush();
    expect(killed).toEqual([...opened.keys()]);
  });
});
