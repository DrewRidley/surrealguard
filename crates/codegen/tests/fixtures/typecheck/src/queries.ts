// The host file `generate` scans. Every string literal handed to `db.query`,
// `defineQuery` or `defineLive` becomes one registry row keyed by its exact
// text; a template's `${...}` becomes a `__hostN` substitution.
//
// This file is fixture input for the Rust golden test, never compiled itself.
import { createClient, defineLive, defineQuery, type RecordId } from "./surrealql-analyzer.generated";

const db = createClient({ url: "ws://localhost:8000/rpc" });

// Plain rows: a record id, a datetime, a string.
export const roster = () => db.query("SELECT id, name, joined FROM person");

// A record-link param, typed from the schema so it encodes as a link.
export const peopleOf = (team: RecordId<"team">) =>
  db.query("SELECT name FROM person WHERE team = $team", { team });

// `option<string>` as a row field (an optional key) …
export const nicks = () => db.query("SELECT name, nick FROM person");
// … and as a whole result (a union with `undefined`).
export const janesNick = () => db.query("SELECT VALUE nick FROM ONLY person:jane");

// A literal-union field, an array field, and a param constrained to the union.
export const byStatus = (status: "active" | "retired") =>
  db.query("SELECT name, status, tags FROM person WHERE status = $status", { status });

// The SDK value classes and an optional record link inside a closed object row.
export const profile = (id: RecordId<"person">) =>
  db.query("SELECT settings, mentor, external_id, tenure FROM person WHERE id = $id", { id });

// Decimal.
export const budgets = () => db.query("SELECT name, budget FROM team");

// Graph traversal off a record, and the edge table's own rows.
export const friends = () => db.query("SELECT ->knows->person.name AS friends FROM person:jane");
export const edges = () => db.query("SELECT in, out, since, weight FROM knows");

// A `fn::` call as the whole result.
export const greeting = (name: string) => db.query("RETURN fn::greet($name)", { name });

// Two statements: the LET responds with null, so the tuple has two slots.
export const recent = () =>
  db.query("LET $cutoff = time::now() - 1w; SELECT name, joined FROM person WHERE joined > $cutoff");

// An aggregate.
export const headcount = () => db.query("SELECT count() AS n FROM person GROUP ALL");

// A template substitution: the key carries `${}` and the params tuple types it.
export const olderThan = (min: number) => db.query(`SELECT name FROM person WHERE age > ${min}`);

// The query-value forms are extracted too.
export const allPeople = defineQuery("SELECT id, name, age FROM person");
export const livePeople = defineLive("SELECT id, name, status FROM person");
