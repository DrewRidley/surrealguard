// Headless proof: instantiate the analyzer WASM under Node and print the
// diagnostics for a canonical unknown-field query.
//
//   node web/public/playground/harness.mjs
//
// Expected: an E1002 (unknown field) finding for `ssn`.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { createAnalyzer } from "./sg-analyzer.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const wasmBytes = readFileSync(join(here, "surrealguard_wasm.wasm"));

const analyzer = await createAnalyzer(wasmBytes);

const schema =
  "DEFINE TABLE user SCHEMAFULL; DEFINE FIELD name ON user TYPE string;";
const query = "SELECT ssn FROM user";

const diagnostics = analyzer.analyze(schema, query);

console.log("schema:", schema);
console.log("query :", query);
console.log("diagnostics:", JSON.stringify(diagnostics, null, 2));

const hasE1002 = diagnostics.some((d) => d.code === "E1002");
if (!hasE1002) {
  console.error("\nFAIL: expected an E1002 unknown-field diagnostic");
  process.exit(1);
}
console.log("\nPASS: E1002 unknown-field diagnostic present");
