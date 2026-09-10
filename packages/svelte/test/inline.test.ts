import { describe, expect, it } from "vitest";
import { HOST_PARAM_PREFIX, sgLive, sgQuery } from "../src/inline.js";
import { surrealguard } from "../src/preprocess.js";

const run = (content: string) => surrealguard().markup({ content, filename: "T.svelte" })?.code;

describe("the inline query attribute", () => {
  it("captures the parts, so the value never reaches the text", () => {
    const query = sgQuery(["SELECT * FROM person WHERE age > ", ""], [30]);
    expect(query.text).toBe("SELECT * FROM person WHERE age > $__host0");
    expect(query.params).toEqual({ __host0: 30 });
  });

  it("keeps a quote inside a value out of the query text", () => {
    // The whole reason the preprocessor exists: string concatenation would
    // splice this in, and `name = 'O'Hara'` is a syntax error at best.
    const query = sgQuery(["SELECT * FROM person WHERE name = ", ""], ["O'Hara"]);
    expect(query.text).toBe("SELECT * FROM person WHERE name = $__host0");
    expect(query.text).not.toContain("O'Hara");
    expect(query.params).toEqual({ __host0: "O'Hara" });
  });

  it("numbers left to right, zero-based", () => {
    const query = sgQuery(["a ", " b ", " c"], [1, 2]);
    expect(query.text).toBe("a $__host0 b $__host1 c");
    expect(query.params).toEqual({ __host0: 1, __host1: 2 });
  });

  it("agrees with the prefix the type level spells out", () => {
    // `Skeleton` writes `$__host` as a literal because a type cannot read a
    // const. If this constant ever moves, that type must move with it.
    expect(HOST_PARAM_PREFIX).toBe("__host");
  });

  it("takes no parameters when nothing is interpolated", () => {
    const query = sgQuery(["SELECT * FROM person"], []);
    expect(query.text).toBe("SELECT * FROM person");
    expect(query.params).toBeUndefined();
  });

  it("builds a live query for <LiveQuery>", () => {
    const live = sgLive(["SELECT * FROM person WHERE team = ", ""], ["team:red"]);
    expect(live.isLive).toBe(true);
    expect(live.text).toBe("SELECT * FROM person WHERE team = $__host0");
    expect(live.liveText).toBe("LIVE SELECT * FROM person WHERE team = $__host0");
  });

  it("gives one cache key per skeleton, not one per value", () => {
    const a = sgQuery(["SELECT * FROM person WHERE age > ", ""], [30]);
    const b = sgQuery(["SELECT * FROM person WHERE age > ", ""], [31]);
    // Different bindings of the SAME query: the keys differ, but both are
    // bindings of one text, which is what `invalidate` matches on by prefix.
    expect(a.key).not.toBe(b.key);
    expect(a.key.startsWith(a.text)).toBe(true);
    expect(b.key.startsWith(a.text)).toBe(true);
  });
});

describe("the preprocessor", () => {
  it("rewrites an interpolated q into a thunk over the parts", () => {
    const out = run(`<script lang="ts">let minAge = 30;</script>
<Query q="SELECT id FROM person WHERE age > {minAge}"></Query>`);
    expect(out).toContain(
      'q={() => __sg_query(["SELECT id FROM person WHERE age > ", ""], [minAge])}',
    );
    expect(out).toContain('from "@surrealguard/svelte/inline"');
  });

  it("wraps it in a thunk, which is what keeps it reactive", () => {
    const out = run(`<script></script><Query q="SELECT * FROM a WHERE b = {c}"></Query>`);
    expect(out).toContain("q={() =>");
  });

  it("uses the live helper for <LiveQuery>", () => {
    const out = run(`<script></script><LiveQuery q="SELECT * FROM p WHERE t = {t}"></LiveQuery>`);
    expect(out).toContain('__sg_live(["SELECT * FROM p WHERE t = ", ""], [t])');
  });

  it("handles a query with no interpolation at all", () => {
    const out = run(`<script></script><Query q="SELECT * FROM person"></Query>`);
    expect(out).toContain('q={() => __sg_query(["SELECT * FROM person"], [])}');
  });

  it("leaves q={expression} alone", () => {
    const source = `<script>let x;</script><Query q={x}></Query>`;
    expect(run(source)).toBeUndefined();
  });

  it("leaves a q attribute on anything else alone", () => {
    const source = `<script></script><div q="SELECT * FROM person"></div>`;
    expect(run(source)).toBeUndefined();
  });

  it("reaches a <LiveQuery> nested inside an {#each}", () => {
    const out = run(`<script>let rows = [];</script>
{#each rows as row}
  <LiveQuery q="SELECT * FROM person WHERE team = {row.team}"></LiveQuery>
{/each}`);
    expect(out).toContain('__sg_live(["SELECT * FROM person WHERE team = ", ""], [row.team])');
  });

  it("returns the file untouched when it cannot be parsed", () => {
    expect(run("<Query q=\"SELECT {\"")).toBeUndefined();
  });
});
