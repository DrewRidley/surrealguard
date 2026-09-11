// @vitest-environment node
/**
 * Server-side rendering. This file runs in the `node` environment on purpose:
 * Vitest transforms it through Vite's SSR pipeline, so the Svelte plugin
 * compiles the components with `generate: "server"` and `render` from
 * `svelte/server` gets the real server output — not the DOM build with a fake
 * document under it.
 *
 * What is under test is the property `preload` exists for: a `<Query>` handed a
 * `Preloaded` payload must emit the rows **in the server's HTML**, so the
 * client hydrates onto markup that is already correct. Anything less is the
 * flash the SSR path exists to remove.
 */
import { describe, expect, it } from "vitest";
import { render } from "svelte/server";
import { preload, type SurrealQLAnalyzerClient } from "@surrealdb/analyzer-client";
import { RecordId } from "surrealdb";
import QueryStates from "./QueryStates.svelte";
import { liveUsers } from "./queries.js";

const CLIENT_KEY = Symbol.for("@surrealdb/analyzer-svelte:client");

function makeClient(rows: Array<{ id: RecordId<"user">; name: string }>) {
  let calls = 0;
  const client = {
    query: async () => {
      calls += 1;
      return [rows];
    },
    surreal: {},
    onInvalidate: () => () => {},
    runLiveOnce: async () => rows,
  } as unknown as SurrealQLAnalyzerClient;
  return { client, calls: () => calls };
}

describe("SSR", () => {
  it("renders a Preloaded payload's rows into the server HTML", async () => {
    const { client, calls } = makeClient([{ id: new RecordId("user", 1), name: "ada" }]);
    const preloaded = await preload(client, liveUsers);
    const before = calls();

    const { body } = render(QueryStates, {
      props: { preloaded },
      context: new Map([[CLIENT_KEY, client]]),
    });

    expect(body).toContain("ada");
    expect(body).not.toContain("loading");
    // The seed is adopted, not re-fetched — the whole point of carrying the key.
    expect(calls()).toBe(before);
  });

  it("renders the `loading` snippet for a query with nothing seeded", () => {
    const { client } = makeClient([]);

    const { body } = render(QueryStates, { context: new Map([[CLIENT_KEY, client]]) });

    expect(body).toContain("loading");
  });
});
