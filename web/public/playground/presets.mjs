// The curated schema + queries both playground surfaces load from.
//
// One module, two consumers (the landing-page demo and /playground/), because
// a preset that only exists on one page is a preset that only gets checked on
// one page. Every entry here is verified twice: `harness.mjs` asserts each
// `expect` against the shipped `.wasm`, and each statement has been run against
// a real `surreal` server — the whole claim of this section is "real
// diagnostics from the actual engine", which a preset SurrealDB itself would
// refuse to parse quietly destroys.
//
// `expect` is the contract, not a comment:
//   - `[]`      the analyzer must find nothing at all
//   - `["Exxxx"]` exactly those codes must be raised (order-insensitive)
// A preset advertising an error and not raising it is a bug in this file or in
// the engine; either way the harness fails rather than the page lying.

/**
 * The shared schema. Deliberately small but *related*: two record tables and
 * two `TYPE RELATION` edges, so the graph presets have somewhere to walk.
 * `email` is `option<string>` so an inferred type has an optional member to
 * show — the folded `option<…>` sugar hides exactly the case a caller has to
 * handle.
 */
export const SCHEMA = `DEFINE TABLE user SCHEMAFULL;
DEFINE FIELD name ON user TYPE string;
DEFINE FIELD age ON user TYPE int;
DEFINE FIELD email ON user TYPE option<string>;

DEFINE TABLE post SCHEMAFULL;
DEFINE FIELD title ON post TYPE string;
DEFINE FIELD author ON post TYPE record<user>;

DEFINE TABLE follows TYPE RELATION IN user OUT user SCHEMAFULL;
DEFINE FIELD since ON follows TYPE datetime;

DEFINE TABLE wrote TYPE RELATION IN user OUT post;`;

/**
 * @typedef {object} Preset
 * @property {string} label   Button text.
 * @property {string} schema  The schema pane's contents.
 * @property {string} query   The query pane's contents.
 * @property {string[]} expect The finding codes the query must raise, exactly.
 * @property {string} [note]  Why this preset is here, for whoever edits next.
 */

/** @type {Preset[]} */
export const PRESETS = [
  {
    label: "Unknown field",
    schema: SCHEMA,
    query: "SELECT name, ssn FROM user WHERE age > 18;",
    expect: ["E1002"],
    note: "SurrealDB returns NONE for a misspelled field and never complains.",
  },
  {
    label: "Type mismatch",
    schema: SCHEMA,
    query: "UPDATE user SET age = 'old' WHERE name = 'Ada';",
    expect: ["E2001"],
    note: "The server rejects this at write time, per row. SurrealQL Analyzer says so first.",
  },
  {
    label: "Missing field",
    schema: SCHEMA,
    query: "CREATE user SET name = 'Ada';",
    expect: ["E2034"],
    note: "A SCHEMAFULL row must satisfy every required field; `age` has no DEFAULT.",
  },
  {
    label: "Silent comparison",
    schema: SCHEMA,
    query: "SELECT age > name FROM user WHERE age > 18;",
    expect: ["E2004"],
    note: "SurrealDB orders across kinds instead of failing, so this is always a valid bool.",
  },
  {
    label: "Graph edge",
    schema: SCHEMA,
    query: "RELATE user:ada->follows->user:grace SET since = time::now();",
    expect: [],
    note: "Valid. The type chip is the edge row — in/out resolved from the RELATION table.",
  },
  {
    label: "Wrong edge",
    schema: SCHEMA,
    query: "RELATE user:ada->follows->post:notes SET since = time::now();",
    expect: ["E3002"],
    note: "`follows` runs user -> user. Nothing but the schema can know this is wrong.",
  },
  {
    label: "Multi-hop",
    schema: SCHEMA,
    query: "SELECT name, ->follows->user->wrote->post.title AS reading FROM user WHERE age > 18;",
    expect: [],
    note: "Two hops and a field, resolved through the schema to array<string>.",
  },
  {
    label: "Inferred types",
    schema: SCHEMA,
    query: `SELECT name, email FROM user WHERE age > 18;
RELATE user:ada->follows->user:grace SET since = time::now();
SELECT ->wrote->post.title AS posts FROM user WHERE age > 18;`,
    expect: [],
    note: "Three statements, three response types — what `surrealql-analyzer generate` emits.",
  },
  {
    label: "Syntax error",
    schema: SCHEMA,
    query: "SELECT name FROM user WHERE (age > 18;",
    expect: ["S0002"],
    note: "The unclosed paren. SurrealDB rejects the whole request.",
  },
  {
    label: "Valid",
    schema: SCHEMA,
    query: "SELECT name, email FROM user WHERE age > 18;",
    expect: [],
    note: "Zero findings. `email` is optional, and the chip spells the `none` out.",
  },
];
