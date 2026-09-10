// The codegen golden, consumed end to end. `test-d/gen/surrealguard.generated.ts`
// is real `surrealguard generate` output over the fixture workspace in
// `crates/codegen/tests/fixtures/typecheck` (pinned byte-for-byte by
// `cargo test -p surrealguard-codegen --test golden`). This file is compiled by
// `tsc` as part of `pnpm typecheck` — so the `expectTypeOf` lines below are
// checked, not merely executed — and run by vitest for the runtime half.
import { describe, expect, expectTypeOf, it } from "vitest";
import {
  createClient,
  Decimal,
  defineLive,
  defineQuery,
  Duration,
  RecordId,
  Uuid,
} from "../test-d/gen/surrealguard.generated.js";
import type { ParamsOf, QueryResultOf, ResultOf, SurqlRegistry } from "../src/index.js";

describe("the generated golden", () => {
  it("re-exports the client entry points and the SDK value classes", () => {
    const db = createClient({ url: "ws://localhost:8000/rpc" });
    expect(typeof db.query).toBe("function");
    expect(typeof db.run).toBe("function");
    // Constructing a param needs only the generated import, and the class is
    // the SDK's: it renders as a record link, not as a quoted string.
    expect(new RecordId("team", "red").toString()).toBe("team:red");
    expect(typeof Decimal).toBe("function");
    expect(typeof Duration).toBe("function");
    expect(typeof Uuid).toBe("function");
  });

  it("keys each fixture query by its exact text", () => {
    // Every literal here must be a registry key, or `defineQuery` rejects it at
    // compile time — that *is* the runtime shape of the registry.
    const roster = defineQuery("SELECT id, name, joined FROM person");
    expect(roster.text).toBe("SELECT id, name, joined FROM person");
    expect(roster.key).toBe("SELECT id, name, joined FROM person");
    expect(roster.isLive).toBe(false);

    const live = defineLive("SELECT id, name, status FROM person");
    expect(live.liveText).toBe("LIVE SELECT id, name, status FROM person");
    expect(live.key).toBe("SELECT id, name, status FROM person");

    const bound = defineQuery("SELECT name FROM person WHERE team = $team").with({
      team: new RecordId("team", "red"),
    });
    expect(bound.params).toEqual({ team: new RecordId("team", "red") });
  });
});

// Type-level assertions. Compiling this block is the test; vitest treats
// `expectTypeOf` as a no-op at runtime.
type Registry = SurqlRegistry;

// Each SDK value class lands where the schema declares it, `option<T>` is an
// optional key inside a row, and a closed object field is a closed object.
expectTypeOf<
  Registry["SELECT settings, mentor, external_id, tenure FROM person WHERE id = $id"]["result"]
>().toEqualTypeOf<
  [
    Array<{
      external_id: Uuid;
      mentor?: RecordId<"person">;
      settings: { locale?: string; notify: boolean; timezone: string };
      tenure: Duration;
    }>,
  ]
>();
expectTypeOf<ResultOf<"SELECT name, budget FROM team">>().toEqualTypeOf<
  [Array<{ budget: Decimal; name: string }>]
>();

// A literal-union field constrains both the row and the param.
expectTypeOf<ResultOf<"SELECT name, status, tags FROM person WHERE status = $status">>().toEqualTypeOf<
  [Array<{ name: string; status: "active" | "retired"; tags: Array<string> }>]
>();
expectTypeOf<
  ParamsOf<"SELECT name, status, tags FROM person WHERE status = $status">
>().toEqualTypeOf<{ status: "active" | "retired" }>();

// Two statements, two slots: the LET contributes `null`.
expectTypeOf<
  ResultOf<"LET $cutoff = time::now() - 1w; SELECT name, joined FROM person WHERE joined > $cutoff">
>().toEqualTypeOf<[null, Array<{ joined: Date; name: string }>]>();

// Graph traversal and the edge table's implicit `in`/`out` links.
expectTypeOf<ResultOf<"SELECT ->knows->person.name AS friends FROM person:jane">>().toEqualTypeOf<
  [Array<{ friends: Array<string> }>]
>();
expectTypeOf<ResultOf<"SELECT in, out, since, weight FROM knows">>().toEqualTypeOf<
  [Array<{ in: RecordId<"person">; out: RecordId<"person">; since: Date; weight: number }>]
>();

// A `fn::` call resolves to its declared return type.
expectTypeOf<ResultOf<"RETURN fn::greet($name)">>().toEqualTypeOf<[string]>();
expectTypeOf<ParamsOf<"RETURN fn::greet($name)">>().toEqualTypeOf<{ name: string }>();

// A text that is not in the registry degrades to the SDK's `unknown[]`, never
// to `any`.
expectTypeOf<QueryResultOf<"SELECT nope FROM nowhere">>().toEqualTypeOf<unknown[]>();

// Pure type checks against the client — `main` is never called.
async function main() {
  const db = createClient({ url: "ws://localhost:8000/rpc" });
  const [teams] = await db.query("SELECT name, budget FROM team");
  expectTypeOf(teams).toEqualTypeOf<Array<{ budget: Decimal; name: string }>>();

  // @ts-expect-error the text reads $status, so the params object is required.
  await db.query("SELECT name, status, tags FROM person WHERE status = $status");
  await db.query("SELECT name, status, tags FROM person WHERE status = $status", {
    // @ts-expect-error "hired" is outside the field's literal union.
    status: "hired",
  });
}
void main;
