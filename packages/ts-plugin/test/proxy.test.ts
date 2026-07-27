/**
 * The proxy, driven against a real TypeScript language service.
 *
 * Not a tsserver — the point of these is to pin the *contract* the proxy has
 * with the service it wraps (pass-through, span arithmetic, silence without a
 * config) fast enough to run on every commit. Driving a real tsserver is the
 * other proof and it lives outside the test suite.
 */

import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import ts from "typescript";
import { beforeEach, describe, expect, it } from "vitest";

import { DIAGNOSTIC_SOURCE, toDiagnosticCode } from "../src/classify";
import { createProxy, WASM_PATH } from "../src/plugin";
import { resetProjectCache } from "../src/project";

const SCHEMA = `DEFINE TABLE account SCHEMAFULL;
DEFINE FIELD username ON account TYPE string;
DEFINE FIELD email ON account TYPE string;
`;

const PREAMBLE = `declare const db: { query(q: string): Promise<unknown> };\n`;

interface Fixture {
  root: string;
  file: string;
  text: string;
  service: ts.LanguageService;
}

/** A one-file project, with or without a `surrealguard.toml` above it. */
function fixture(body: string, options: { config?: boolean } = {}): Fixture {
  const root = mkdtempSync(join(tmpdir(), "sg-proxy-"));
  mkdirSync(join(root, "schema"), { recursive: true });
  mkdirSync(join(root, "src"), { recursive: true });
  writeFileSync(join(root, "schema", "account.surql"), SCHEMA);
  if (options.config !== false) writeFileSync(join(root, "surrealguard.toml"), "[sources]\n");

  const file = join(root, "src", "app.ts");
  const text = PREAMBLE + body;
  writeFileSync(file, text);

  const files = new Map([[file, text]]);
  const host: ts.LanguageServiceHost = {
    getScriptFileNames: () => [...files.keys()],
    getScriptVersion: () => "1",
    getScriptSnapshot: (name) => {
      const content = files.get(name) ?? ts.sys.readFile(name);
      return content === undefined ? undefined : ts.ScriptSnapshot.fromString(content);
    },
    getCurrentDirectory: () => root,
    getCompilationSettings: () => ({ target: ts.ScriptTarget.ES2022, strict: true }),
    getDefaultLibFileName: (settings) => ts.getDefaultLibFilePath(settings),
    fileExists: ts.sys.fileExists,
    readFile: ts.sys.readFile,
    readDirectory: ts.sys.readDirectory,
    directoryExists: ts.sys.directoryExists,
    getDirectories: ts.sys.getDirectories,
  };

  const service = ts.createLanguageService(host, ts.createDocumentRegistry());
  const info = {
    languageService: service,
    languageServiceHost: host,
    project: { projectService: { logger: { info: () => {} } } },
    serverHost: {},
    config: {},
  } as unknown as ts.server.PluginCreateInfo;

  return { root, file, text, service: createProxy(ts, info, WASM_PATH) };
}

beforeEach(() => resetProjectCache());

describe("getSemanticDiagnostics", () => {
  it("puts a finding on the offending token inside the string literal", () => {
    const { file, text, service } = fixture(
      `export const rows = db.query("SELECT username, nope FROM account");\n`,
    );
    const diagnostics = service.getSemanticDiagnostics(file);
    const mine = diagnostics.filter((d) => d.source === DIAGNOSTIC_SOURCE);
    const unknown = mine.find((d) => d.code === toDiagnosticCode("E1002"));
    expect(unknown, JSON.stringify(mine.map((d) => d.messageText))).toBeDefined();
    // The whole contract: slice the file at the span and get the token.
    expect(text.slice(unknown!.start!, unknown!.start! + unknown!.length!)).toBe("nope");
    expect(unknown!.category).toBe(ts.DiagnosticCategory.Error);
    expect(unknown!.file?.fileName).toBe(file);
  });

  it("maps severity onto TypeScript's three categories", () => {
    const { file, service } = fixture(
      `export const rows = db.query("SELECT * FROM account");\n`,
    );
    const mine = service
      .getSemanticDiagnostics(file)
      .filter((d) => d.source === DIAGNOSTIC_SOURCE);
    // `SELECT *` is a lint, and a lint is a suggestion — not an error the
    // editor paints red in code that runs perfectly well.
    expect(mine.some((d) => d.category === ts.DiagnosticCategory.Suggestion)).toBe(true);
  });

  it("keeps TypeScript's own diagnostics", () => {
    const { file, service } = fixture(
      `const n: number = "no";\nexport const rows = db.query("SELECT nope FROM account");\n`,
    );
    const diagnostics = service.getSemanticDiagnostics(file);
    // 2322: Type 'string' is not assignable to type 'number'.
    expect(diagnostics.some((d) => d.code === 2322)).toBe(true);
    expect(diagnostics.some((d) => d.source === DIAGNOSTIC_SOURCE)).toBe(true);
  });

  it("says nothing at all without a surrealguard.toml", () => {
    const { file, service } = fixture(
      `export const rows = db.query("SELECT nope FROM nothing_at_all");\n`,
      { config: false },
    );
    expect(
      service.getSemanticDiagnostics(file).filter((d) => d.source === DIAGNOSTIC_SOURCE),
    ).toEqual([]);
  });
});

describe("getEncodedSemanticClassifications", () => {
  function classify(fixtureResult: Fixture, format: ts.SemanticClassificationFormat) {
    const { file, text, service } = fixtureResult;
    const encoded = service.getEncodedSemanticClassifications(
      file,
      { start: 0, length: text.length },
      format,
    );
    const out: Array<[string, number]> = [];
    for (let i = 0; i < encoded.spans.length; i += 3) {
      out.push([
        text.slice(encoded.spans[i] as number, (encoded.spans[i] as number) + (encoded.spans[i + 1] as number)),
        encoded.spans[i + 2] as number,
      ]);
    }
    return out;
  }

  it("classifies the query's identifiers in the 2020 format", () => {
    const result = fixture(
      `export const rows = db.query("SELECT username FROM account");\n`,
    );
    const spans = classify(result, ts.SemanticClassificationFormat.TwentyTwenty);
    const covered = spans.map(([text]) => text);
    expect(covered).toContain("username");
    expect(covered).toContain("account");
    // No lexical kinds: the 2020 legend has no `keyword`, so SELECT is left
    // as TypeScript coloured it rather than painted as something it is not.
    expect(covered).not.toContain("SELECT");
  });

  it("classifies keywords too in the Original format", () => {
    const result = fixture(
      `export const rows = db.query("SELECT username FROM account");\n`,
    );
    const spans = classify(result, ts.SemanticClassificationFormat.Original);
    const keyword = spans.find(([text]) => text === "SELECT");
    expect(keyword).toBeDefined();
    expect(keyword![1]).toBe(ts.ClassificationType.keyword);
  });

  it("returns spans in ascending order", () => {
    // Clients walk these pushing deltas; one out of order moves everything
    // after it to the wrong place on screen.
    const result = fixture(
      `export const a = db.query("SELECT username FROM account");\n` +
        `export const b = db.query("SELECT email FROM account");\n`,
    );
    const { file, text, service } = result;
    const encoded = service.getEncodedSemanticClassifications(
      file,
      { start: 0, length: text.length },
      ts.SemanticClassificationFormat.TwentyTwenty,
    );
    const starts: number[] = [];
    for (let i = 0; i < encoded.spans.length; i += 3) starts.push(encoded.spans[i] as number);
    expect(starts).toEqual([...starts].sort((x, y) => x - y));
  });

  it("adds nothing without a surrealguard.toml", () => {
    const withConfig = classify(
      fixture(`export const rows = db.query("SELECT username FROM account");\n`),
      ts.SemanticClassificationFormat.TwentyTwenty,
    );
    resetProjectCache();
    const without = classify(
      fixture(`export const rows = db.query("SELECT username FROM account");\n`, {
        config: false,
      }),
      ts.SemanticClassificationFormat.TwentyTwenty,
    );
    expect(withConfig.length).toBeGreaterThan(without.length);
    expect(without.map(([text]) => text)).not.toContain("username");
  });
});

describe("getQuickInfoAtPosition", () => {
  it("reports the field type under the cursor inside a query", () => {
    const { file, text, service } = fixture(
      `export const rows = db.query("SELECT username FROM account");\n`,
    );
    const info = service.getQuickInfoAtPosition(file, text.indexOf("username") + 2);
    expect(info?.documentation?.[0]?.text).toContain("string");
    expect(text.slice(info!.textSpan.start, info!.textSpan.start + info!.textSpan.length)).toBe(
      "username",
    );
  });

  it("leaves TypeScript's own hovers alone", () => {
    const { file, text, service } = fixture(
      `export const rows = db.query("SELECT username FROM account");\n`,
    );
    const info = service.getQuickInfoAtPosition(file, text.indexOf("rows") + 1);
    expect(info?.displayParts?.map((p) => p.text).join("")).toContain("rows");
  });
});

describe("the proxy itself", () => {
  it("forwards every method it does not override", () => {
    const { file, service } = fixture(`export const rows = 1;\n`);
    // A method nobody here has thought about still has to work, or installing
    // the plugin silently removes editor features.
    expect(service.getCompletionsAtPosition(file, 0, undefined)).toBeDefined();
    expect(service.getProgram()?.getSourceFile(file)).toBeDefined();
  });
});
