// Headless proof: instantiate the analyzer WASM under Node and check that the
// bundle actually on disk behaves the way the playground claims it does.
//
//   node web/public/playground/harness.mjs
//
// This runs the *shipped* `.wasm`, not the Rust source, which is the only way
// to catch the failure that matters here: the bundle in `web/public/` drifting
// behind the crate that produced it. Every preset is asserted — a preset
// advertising an error must raise it, and one labelled valid must find nothing
// — because a playground whose examples do not do what their label says is
// worse than no playground.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { createAnalyzer } from "./sg-analyzer.mjs";
import { PRESETS } from "./presets.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const wasmBytes = readFileSync(join(here, "surrealguard_wasm.wasm"));

const analyzer = await createAnalyzer(wasmBytes);

let failures = 0;
const fail = (msg) => {
  console.error(`FAIL: ${msg}`);
  failures++;
};

// --- the canonical smoke test ------------------------------------------------
{
  const diagnostics = analyzer.analyze(
    "DEFINE TABLE user SCHEMAFULL; DEFINE FIELD name ON user TYPE string;",
    "SELECT ssn FROM user"
  );
  if (!diagnostics.some((d) => d.code === "E1002")) {
    fail(`expected an E1002 unknown-field diagnostic, got ${JSON.stringify(diagnostics)}`);
  } else {
    console.log("ok  E1002 unknown-field diagnostic present");
  }
}

// --- the type-chip ABI -------------------------------------------------------
// Feature-detected in the browser, required here: this harness exists to prove
// the shipped bundle is the new one.
if (!analyzer.hasTypes) {
  fail("the bundle does not export sg_analyze2 — rebuild with crates/wasm/build-wasm.sh");
} else {
  const { statements } = analyzer.analyzeWithTypes(
    "DEFINE TABLE user SCHEMAFULL; DEFINE FIELD name ON user TYPE string;",
    "SELECT name FROM user WHERE name != '';"
  );
  if (statements.length !== 1) {
    fail(`expected one statement, got ${JSON.stringify(statements)}`);
  } else if (statements[0].response !== "array<{ name: string }>") {
    fail(`unexpected response kind: ${JSON.stringify(statements[0])}`);
  } else if (statements[0].start !== 0) {
    fail(`statement spans must be query-relative: ${JSON.stringify(statements[0])}`);
  } else {
    console.log(`ok  sg_analyze2 -> ${statements[0].response}`);
  }
}

// --- every preset does what its label says -----------------------------------
const isSyntaxCode = (code) => /^S\d{4}$/.test(code);

for (const preset of PRESETS) {
  // Same two passes the page runs: syntax comes from a schema-free pass, or
  // tree-sitter's error recovery can swallow the query into the schema prefix.
  const first = analyzer.analyzeWithTypes(preset.schema, preset.query);
  const semantic = first.diagnostics.filter((d) => !isSyntaxCode(d.code));
  const syntax = analyzer.analyze("", preset.query).filter((d) => isSyntaxCode(d.code));
  const schemaPane = analyzer.analyze("", preset.schema);

  if (schemaPane.length) {
    fail(`preset "${preset.label}": the schema itself raises ${JSON.stringify(schemaPane)}`);
  }

  const got = [...semantic, ...syntax].map((d) => d.code).sort();
  const want = [...preset.expect].sort();
  if (got.join(",") !== want.join(",")) {
    fail(`preset "${preset.label}": expected [${want}] but got [${got}]`);
    continue;
  }

  // A preset with no errors is the one that shows off inference, so it had
  // better have something to show: every statement must carry a type.
  if (!preset.expect.length && analyzer.hasTypes) {
    const untyped = first.statements.filter((s) => !s.response);
    if (untyped.length) {
      fail(`preset "${preset.label}": statement without a response kind: ${JSON.stringify(untyped)}`);
      continue;
    }
  }

  const types = first.statements.map((s) => s.response).join("  ·  ");
  console.log(`ok  ${preset.label.padEnd(18)} [${got}]${types ? `  ${types}` : ""}`);
}

if (failures) {
  console.error(`\n${failures} failure(s)`);
  process.exit(1);
}
console.log("\nPASS: all presets behave as advertised");
