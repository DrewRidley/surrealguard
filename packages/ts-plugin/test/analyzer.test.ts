import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { loadAnalyzer } from "../src/analyzer";
import { byteToUtf16, utf16ToByte } from "../src/offsets";
import {
  DIAGNOSTIC_CODE_BASE,
  toDiagnosticCode,
  toOriginal,
  toTwentyTwenty,
  TokenKind,
} from "../src/classify";
import { findProject, findRoot, resetProjectCache } from "../src/project";
import { mergeSpans, WASM_PATH } from "../src/plugin";

const SCHEMA =
  "DEFINE TABLE account SCHEMAFULL;\nDEFINE FIELD username ON account TYPE string;\n";

const analyzer = loadAnalyzer(WASM_PATH);

describe("the analyzer", () => {
  it("reports a finding at the host bytes it is about", () => {
    const source = `const q = db.query("SELECT username, nope FROM account");\n`;
    const { diagnostics } = analyzer.analyzeHost({
      schema: SCHEMA,
      file_name: "app.ts",
      source,
    });
    const unknown = diagnostics.find((d) => d.code === "E1002");
    expect(unknown, JSON.stringify(diagnostics)).toBeDefined();
    // The proof is the slice, not the presence: a span that covers the whole
    // string literal would also "have a finding".
    expect(source.slice(unknown!.start, unknown!.end)).toBe("nope");
    expect(unknown!.severity).toBe("error");
  });

  it("classifies the query's tokens and nothing outside it", () => {
    const source = `const q = db.query("SELECT username FROM account");\n`;
    const { tokens } = analyzer.analyzeHost({
      schema: SCHEMA,
      file_name: "app.ts",
      source,
    });
    expect(tokens.map((t) => source.slice(t.start, t.end))).toEqual([
      "SELECT",
      "username",
      "FROM",
      "account",
    ]);
    // `const`, `q`, `db` and `query` are TypeScript's business.
    const openQuote = source.indexOf('"');
    expect(tokens.every((t) => t.start > openQuote)).toBe(true);
  });

  it("answers a hover with the field's declared type", () => {
    const source = `const q = db.query("SELECT username FROM account");\n`;
    const { hover } = analyzer.analyzeHost({
      schema: SCHEMA,
      file_name: "app.ts",
      source,
      hover: source.indexOf("username") + 2,
    });
    expect(hover?.markdown).toContain("string");
    expect(source.slice(hover!.start, hover!.end)).toBe("username");
  });

  it("says nothing about a file with no query in it", () => {
    const result = analyzer.analyzeHost({
      schema: SCHEMA,
      file_name: "app.ts",
      source: "export const answer = 42;\n",
    });
    expect(result).toEqual({ diagnostics: [], tokens: [], queries: [], hover: null });
  });
});

describe("offset conversion", () => {
  it("is the identity for ASCII", () => {
    const text = "SELECT name FROM person";
    const toUtf16 = byteToUtf16(text);
    expect(toUtf16(7)).toBe(7);
    expect(utf16ToByte(text)(7)).toBe(7);
  });

  it("keeps a span on its token past a multi-byte character", () => {
    // 'é' is two UTF-8 bytes and one UTF-16 unit; the emoji is four and two.
    // A plugin that skips this puts every later squiggle three columns right.
    const text = "-- é😀\nSELECT name";
    const target = text.indexOf("name");
    const targetByte = Buffer.byteLength(text.slice(0, target), "utf8");
    expect(targetByte).not.toBe(target);
    expect(byteToUtf16(text)(targetByte)).toBe(target);
    expect(utf16ToByte(text)(target)).toBe(targetByte);
  });

  it("round-trips every position in a mixed-width string", () => {
    const text = "aé😀b\nc€d";
    const toByte = utf16ToByte(text);
    const toUtf16 = byteToUtf16(text);
    for (let unit = 0; unit <= text.length; unit++) {
      // Positions inside a surrogate pair have no byte of their own; the
      // converter answers with the character's start, which is the only
      // position that names a real boundary.
      const roundTrip = toUtf16(toByte(unit));
      expect(roundTrip).toBeLessThanOrEqual(unit);
      expect(unit - roundTrip).toBeLessThanOrEqual(1);
    }
  });
});

describe("classification", () => {
  it("gives every kind an Original-format classification", () => {
    for (const kind of Object.values(TokenKind).filter(
      (value): value is TokenKind => typeof value === "number",
    )) {
      expect(toOriginal(kind)).toBeGreaterThan(0);
    }
  });

  it("omits the kinds the 2020 format cannot express, rather than lying", () => {
    // There is no `keyword` token type in the 2020 legend. Mapping SELECT onto
    // some unrelated kind would paint it with the theme's colour for that kind
    // — a wrong answer is worse than TypeScript's own string green.
    expect(toTwentyTwenty(TokenKind.Keyword)).toBeUndefined();
    expect(toTwentyTwenty(TokenKind.String)).toBeUndefined();
    expect(toTwentyTwenty(TokenKind.Comment)).toBeUndefined();
    // The identifier half does map, and is the half that carries meaning.
    expect(toTwentyTwenty(TokenKind.Property)).toBe((9 + 1) << 8);
    expect(toTwentyTwenty(TokenKind.Parameter)).toBe((6 + 1) << 8);
  });

  it("puts finding codes where TypeScript's cannot reach", () => {
    // TypeScript's highest shipped code is five digits; a user filtering on
    // 1001002 can only ever be filtering on ours.
    expect(toDiagnosticCode("E1002")).toBe(1_001_002);
    expect(toDiagnosticCode("L7014")).toBe(1_007_014);
    expect(toDiagnosticCode("S0001")).toBe(1_000_001);
    expect(DIAGNOSTIC_CODE_BASE).toBeGreaterThan(100_000);
  });
});

describe("merging classification spans", () => {
  it("keeps triples in ascending order", () => {
    // A client walks these in order pushing deltas; one triple out of place
    // moves every token after it.
    const merged = mergeSpans([0, 3, 1, 20, 4, 1], [10, 6, 2, 30, 2, 5]);
    expect(merged).toEqual([0, 3, 1, 10, 6, 2, 20, 4, 1, 30, 2, 5]);
  });

  it("keeps a tail from either side", () => {
    expect(mergeSpans([], [5, 1, 1])).toEqual([5, 1, 1]);
    expect(mergeSpans([5, 1, 1], [])).toEqual([5, 1, 1]);
  });
});

describe("project discovery", () => {
  function scaffold(withConfig: boolean): string {
    const root = mkdtempSync(join(tmpdir(), "sg-plugin-"));
    mkdirSync(join(root, "schema"), { recursive: true });
    mkdirSync(join(root, "src"), { recursive: true });
    mkdirSync(join(root, "node_modules", "junk"), { recursive: true });
    writeFileSync(join(root, "schema", "account.surql"), SCHEMA);
    writeFileSync(join(root, "node_modules", "junk", "vendor.surql"), "DEFINE TABLE vendored;");
    if (withConfig) writeFileSync(join(root, "surrealguard.toml"), "[sources]\n");
    resetProjectCache();
    return root;
  }

  it("finds the root by walking up, and loads the schema", () => {
    const root = scaffold(true);
    expect(findRoot(join(root, "src", "app.ts"))).toBe(root);
    const project = findProject(join(root, "src", "app.ts"));
    expect(project?.schema).toContain("DEFINE FIELD username");
  });

  it("skips ignored directories", () => {
    const root = scaffold(true);
    expect(findProject(join(root, "src", "app.ts"))?.schema).not.toContain("vendored");
  });

  it("has no project at all without a surrealguard.toml", () => {
    // This is the whole "degrade to silence" contract. Without the config the
    // plugin has no schema, and analysing against no schema would call every
    // table in the file undefined — noise in a repo that never opted in.
    const root = scaffold(false);
    expect(findRoot(join(root, "src", "app.ts"))).toBeNull();
    expect(findProject(join(root, "src", "app.ts"))).toBeNull();
  });

  it("honours [sources] globs from the config", () => {
    const root = mkdtempSync(join(tmpdir(), "sg-plugin-"));
    mkdirSync(join(root, "vendor"), { recursive: true });
    mkdirSync(join(root, "db"), { recursive: true });
    writeFileSync(join(root, "db", "schema.surql"), SCHEMA);
    writeFileSync(join(root, "vendor", "other.surql"), "DEFINE TABLE vendored;");
    writeFileSync(
      join(root, "surrealguard.toml"),
      '[sources]\nschema = ["db/**/*.surql"]\nignore = ["vendor/**"]\n',
    );
    resetProjectCache();
    const project = findProject(join(root, "app.ts"));
    expect(project?.schema).toContain("account");
    expect(project?.schema).not.toContain("vendored");
  });
});
