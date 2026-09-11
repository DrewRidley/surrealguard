/**
 * A `<Query>` follows its parameters.
 *
 * The interesting case is the one that is easy to assume works: `params` is
 * read inside the same thunk as `q`, and that thunk runs inside a `$derived`,
 * so changing a parameter re-resolves the query and re-runs it. There is no
 * `reactive` opt-in because there is nothing to opt into.
 */
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/svelte";
import type { SurrealQLAnalyzerClient } from "@surrealdb/analyzer-client";
import Params from "./Params.svelte";

const CLIENT_KEY = Symbol.for("@surrealdb/analyzer-svelte:client");
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("<Query> and changing params", () => {
  it("re-runs when a parameter changes", async () => {
    const seen: Array<unknown> = [];
    const query = vi.fn(async (_sql: string, params?: Record<string, unknown>) => {
      seen.push(params?.team);
      return [[{ id: 1, name: "a" }]];
    });
    const client = {
      query,
      surreal: { query },
      onInvalidate: () => () => {},
    } as unknown as SurrealQLAnalyzerClient;

    const { rerender } = render(Params, {
      props: { team: "red" },
      context: new Map([[CLIENT_KEY, client]]),
    });
    await flush();
    await screen.findByTestId("rows");

    await rerender({ team: "blue" });
    await flush();

    expect(seen).toEqual(["red", "blue"]);
  });
});
