import { describe, expect, it } from "vitest";
import { RecordId } from "surrealdb";
import { computeKey, defineLive, defineQuery } from "../src/index.js";
import { reconcile, type ReconcilableRow } from "../src/live.js";

// The registry is empty in this package, so the runtime behaviour is exercised
// through the `unchecked` factories. Type-level behaviour lives in `test-d`.

describe("query references", () => {
  it("carry their text and derive a stable key", () => {
    const q = defineQuery.unchecked("SELECT name FROM person");
    expect(q.text).toBe("SELECT name FROM person");
    expect(q.key).toBe("SELECT name FROM person");
    expect(q.isLive).toBe(false);
  });

  it("prefix LIVE for the subscription but key on the bare text", () => {
    const live = defineLive.unchecked("SELECT name FROM person");
    // The analyzer keyed the bare text, and the SSR seed runs the bare text.
    expect(live.text).toBe("SELECT name FROM person");
    expect(live.liveText).toBe("LIVE SELECT name FROM person");
    expect(live.key).toBe("SELECT name FROM person");
  });

  it("bind params without mutating the original", () => {
    const base = defineQuery.unchecked("SELECT * FROM person WHERE team = $team");
    const bound = base.with({ team: new RecordId("team", "red") });
    expect(base.params).toBeUndefined();
    expect(bound.params).toEqual({ team: new RecordId("team", "red") });
    expect(bound.key).not.toBe(base.key);
  });

  it("key params independently of insertion order", () => {
    expect(computeKey("Q", { a: 1, b: 2 })).toBe(computeKey("Q", { b: 2, a: 1 }));
  });

  it("do NOT collide a record link with the string that looks like it", () => {
    // This is the whole reason the key uses the SDK's renderer: `r"team:red"`
    // and `s"team:red"` are different queries — one matches a record link and
    // the other cannot — so they must not share a cache entry.
    const asRecord = computeKey("Q", { team: new RecordId("team", "red") });
    const asString = computeKey("Q", { team: "team:red" });
    expect(asRecord).not.toBe(asString);
  });
});

describe("reconcile", () => {
  // Typed, or `reconcile([], …)` infers its row type as `never` and the
  // `rows[0]!.name` reads below do not compile.
  const empty: ReconcilableRow[] = [];
  const message = (action: "CREATE" | "UPDATE" | "DELETE", id: string, value = {}) =>
    ({
      queryId: undefined as never,
      action,
      recordId: new RecordId("person", id),
      value,
    }) as never;

  it("appends a created record", () => {
    const rows = reconcile(empty, message("CREATE", "ada", { name: "ada" }));
    expect(rows).toHaveLength(1);
    expect(rows[0]!.name).toBe("ada");
  });

  it("replaces an updated record in place", () => {
    const created = reconcile(empty, message("CREATE", "ada", { name: "ada" }));
    const updated = reconcile(created, message("UPDATE", "ada", { name: "Ada" }));
    expect(updated).toHaveLength(1);
    expect(updated[0]!.name).toBe("Ada");
  });

  it("removes a deleted record", () => {
    const created = reconcile(empty, message("CREATE", "ada", { name: "ada" }));
    expect(reconcile(created, message("DELETE", "ada"))).toHaveLength(0);
  });

  it("ignores KILLED and never mutates the input", () => {
    const rows = reconcile(empty, message("CREATE", "ada", { name: "ada" }));
    const after = reconcile(rows, { action: "KILLED" } as never);
    expect(after).toBe(rows);
  });
});
