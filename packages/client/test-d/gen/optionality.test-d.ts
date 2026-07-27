// One SurrealQL `option<string>`, two TypeScript spellings — and a proof that
// they are not the same type, which is why the emitter has to be told which
// position it is rendering into.
//
// `nick` is `option<string>`. The two registry rows below came from the same
// `Kind`; the only difference is where it sits:
//
//   SELECT name, nick FROM person             → Array<{ name: string; nick?: string }>
//   SELECT VALUE nick FROM ONLY person:jane   → undefined | string
//
// The Rust side pins those exact rows in `crates/codegen/src/registry.rs`
// (`an_optional_field_and_an_optional_result_spell_differently`); this file
// pins what `tsc` then makes of them.
//
// Pure type checks — `main` is never called, so no connection opens.
import { createClient, defineQuery } from "./surrealguard.generated.js";
import type { Equal, Expect } from "../assert.js";

// The two spellings, stated on their own so the claim does not depend on the
// registry resolving: they are *different types*, so picking one by which
// emitter function happened to be running was never safe.
type AsProperty = { nick?: string };
type AsValue = { nick: string | undefined };
type _distinct = Expect<Equal<Equal<AsProperty, AsValue>, false>>;

// And the difference that matters to a caller: an optional key may be omitted.
const omitted: AsProperty = {};
void omitted;
// @ts-expect-error a value union has no key to omit — `nick` must be written
const required: AsValue = {};
void required;

const db = createClient({ url: "ws://localhost:8000/rpc" });
const roster = defineQuery("SELECT name, nick FROM person");
const oneNick = defineQuery("SELECT VALUE nick FROM ONLY person:jane");

async function main() {
  // A row field: the `none` became a key that may be absent.
  const rows = await db.run(roster);
  type Row = (typeof rows)[number];
  type _row = Expect<Equal<Row, { name: string; nick?: string }>>;
  const partial: Row = { name: "jane" };
  partial.nick?.length;
  // The `?` moved the absence to the key; it did not widen the value.
  type _present = Expect<Equal<NonNullable<Row["nick"]>, string>>;
  // @ts-expect-error `name` has no `none` to fold, so it stays required
  const missingName: Row = { nick: "j" };
  void missingName;

  // A statement's whole result: no key exists, so the absence is a union
  // member and the caller has to narrow it.
  const nick = await db.run(oneNick);
  type _value = Expect<Equal<typeof nick, string | undefined>>;
  // @ts-expect-error `string | undefined` has no `.length` until it is checked
  nick.length;
  if (nick !== undefined) nick.length;
}
void main;
