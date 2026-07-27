/**
 * The language-service proxy: SurrealGuard's answers, given as TypeScript's.
 *
 * A TypeScript language service plugin wraps the service the editor already
 * talks to. Every method it does not override passes straight through; the
 * three it does override answer the same question TypeScript was asked, with
 * the embedded SurrealQL folded in. Nothing merges two servers' opinions,
 * because there is only ever one server.
 *
 * # What runs on a keystroke
 *
 * tsserver calls `getSemanticDiagnostics` and `getEncodedSemanticClassifications`
 * constantly and independently. Both are served from **one** analysis per
 * `(file text, schema version)` pair, so the second of the two is free. A file
 * outside a SurrealGuard project short-circuits before any of it: one cached
 * directory lookup and a pass-through.
 *
 * # Failure is silence
 *
 * Every override is wrapped: if the analyzer will not load, if the WASM traps,
 * if a snapshot is missing, the plugin returns exactly what TypeScript said and
 * logs. A plugin loaded into someone's editor must never be able to take their
 * TypeScript with it.
 */

import { join } from "node:path";

import type tsModule from "typescript/lib/tsserverlibrary";

import { loadAnalyzer, type Analyzer, type HostResponse } from "./analyzer";
import {
  DIAGNOSTIC_SOURCE,
  toDiagnosticCode,
  toOriginal,
  toTwentyTwenty,
  type TokenKind,
} from "./classify";
import { byteToUtf16, utf16ToByte, type ByteToUtf16 } from "./offsets";
import { findProject } from "./project";

/** The extensions `surrealguard_embed::extract` knows how to read. */
const HOST_EXTENSIONS = [".ts", ".tsx", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs", ".svelte", ".vue", ".astro"];

/** Where the analyzer lives inside the published package. */
export const WASM_PATH = join(__dirname, "..", "wasm", "surrealguard.wasm");

/** One file's analysis, valid while its text and its schema are unchanged. */
interface Entry {
  text: string;
  schemaVersion: number;
  response: HostResponse;
  toUtf16: ByteToUtf16;
  /** Synthesized only when the program has no real one; see `sourceFileFor`. */
  synthetic?: tsModule.SourceFile;
}

/**
 * Builds the proxy for one project. Exported separately from the plugin entry
 * point so tests can drive it against a hand-built `PluginCreateInfo` without
 * standing up a tsserver.
 */
export function createProxy(
  ts: typeof tsModule,
  info: tsModule.server.PluginCreateInfo,
  wasmPath: string = WASM_PATH,
): tsModule.LanguageService {
  const service = info.languageService;
  const log = (message: string) => {
    try {
      info.project.projectService.logger.info(`[surrealguard] ${message}`);
    } catch {
      // A logger that is not there is not a reason to fail.
    }
  };

  let analyzer: Analyzer | null = null;
  let analyzerFailed = false;
  const cache = new Map<string, Entry>();

  /** The analyzer, loaded on first real need. `null` once loading has failed. */
  function ready(): Analyzer | null {
    if (analyzer || analyzerFailed) return analyzer;
    try {
      analyzer = loadAnalyzer(wasmPath, log);
      log(`analyzer loaded from ${wasmPath}`);
    } catch (error) {
      analyzerFailed = true;
      log(`analyzer unavailable, staying silent: ${String(error)}`);
    }
    return analyzer;
  }

  function isHostFile(fileName: string): boolean {
    return HOST_EXTENSIONS.some((extension) => fileName.endsWith(extension));
  }

  function textOf(fileName: string): string | undefined {
    const snapshot = info.languageServiceHost.getScriptSnapshot(fileName);
    return snapshot?.getText(0, snapshot.getLength());
  }

  /**
   * The analysis for `fileName`, or `undefined` when there is nothing to say —
   * wrong extension, no project, no analyzer, no snapshot.
   *
   * The cheapest checks come first on purpose. In a repo with no
   * `surrealguard.toml` this returns on the second line, having done one
   * cached directory lookup, which is what "costs nothing" has to mean when
   * the caller is a keystroke.
   */
  function analysisOf(fileName: string): Entry | undefined {
    if (!isHostFile(fileName)) return undefined;
    const project = findProject(fileName);
    if (!project) return undefined;

    const text = textOf(fileName);
    if (text === undefined) return undefined;

    const cached = cache.get(fileName);
    if (cached && cached.schemaVersion === project.version && cached.text === text) {
      return cached;
    }

    const engine = ready();
    if (!engine) return undefined;

    let response: HostResponse;
    try {
      response = engine.analyzeHost({
        schema: project.schema,
        file_name: fileName,
        source: text,
      });
    } catch (error) {
      log(`analysis of ${fileName} failed: ${String(error)}`);
      return undefined;
    }

    const entry: Entry = {
      text,
      schemaVersion: project.version,
      response,
      toUtf16: byteToUtf16(text),
    };
    cache.set(fileName, entry);
    return entry;
  }

  /**
   * The `SourceFile` a diagnostic hangs off.
   *
   * For a file in the program that is the program's. For one that is not — a
   * `.svelte` under a TypeScript-only `tsconfig`, say — a synthesized file
   * carrying the same text serves: the only thing anything downstream reads
   * from it is the name and the line map, and refusing to answer because
   * TypeScript does not consider the file its own would silently drop exactly
   * the framework files this plugin exists for.
   */
  function sourceFileFor(fileName: string, entry: Entry): tsModule.SourceFile {
    const real = service.getProgram()?.getSourceFile(fileName);
    if (real && real.text === entry.text) return real;
    entry.synthetic ??= ts.createSourceFile(
      fileName,
      entry.text,
      ts.ScriptTarget.Latest,
      /* setParentNodes */ false,
    );
    return entry.synthetic;
  }

  function category(severity: "error" | "warning" | "hint"): tsModule.DiagnosticCategory {
    switch (severity) {
      case "error":
        return ts.DiagnosticCategory.Error;
      case "warning":
        return ts.DiagnosticCategory.Warning;
      case "hint":
        return ts.DiagnosticCategory.Suggestion;
    }
  }

  const proxy: tsModule.LanguageService = Object.create(null);
  // The documented plugin shape: forward every method, then override.
  for (const key of Object.keys(service) as (keyof tsModule.LanguageService)[]) {
    const member = service[key];
    // @ts-expect-error — a faithful forward is untypeable member-by-member.
    proxy[key] = typeof member === "function" ? member.bind(service) : member;
  }

  proxy.getSemanticDiagnostics = (fileName) => {
    const prior = service.getSemanticDiagnostics(fileName);
    try {
      const entry = analysisOf(fileName);
      if (!entry || entry.response.diagnostics.length === 0) return prior;
      const file = sourceFileFor(fileName, entry);
      const mine = entry.response.diagnostics.map((finding): tsModule.Diagnostic => {
        const start = entry.toUtf16(finding.start);
        return {
          file,
          start,
          length: Math.max(1, entry.toUtf16(finding.end) - start),
          messageText: `${finding.message} (${finding.code})`,
          category: category(finding.severity),
          code: toDiagnosticCode(finding.code),
          source: DIAGNOSTIC_SOURCE,
        };
      });
      return [...prior, ...mine];
    } catch (error) {
      log(`getSemanticDiagnostics(${fileName}) failed: ${String(error)}`);
      return prior;
    }
  };

  proxy.getEncodedSemanticClassifications = (fileName, span, format) => {
    const prior = service.getEncodedSemanticClassifications(fileName, span, format);
    try {
      const entry = analysisOf(fileName);
      if (!entry || entry.response.tokens.length === 0) return prior;

      const twentyTwenty = format === ts.SemanticClassificationFormat.TwentyTwenty;
      const mine: number[] = [];
      for (const token of entry.response.tokens) {
        const start = entry.toUtf16(token.start);
        const length = entry.toUtf16(token.end) - start;
        if (length <= 0) continue;
        if (start >= span.start + span.length || start + length <= span.start) continue;
        const classification = twentyTwenty
          ? toTwentyTwenty(token.kind as TokenKind)
          : toOriginal(token.kind as TokenKind);
        if (classification === undefined) continue;
        mine.push(start, length, classification);
      }
      if (mine.length === 0) return prior;

      return {
        spans: mergeSpans(prior.spans, mine),
        endOfLineState: prior.endOfLineState,
      };
    } catch (error) {
      log(`getEncodedSemanticClassifications(${fileName}) failed: ${String(error)}`);
      return prior;
    }
  };

  proxy.getQuickInfoAtPosition = (fileName, position) => {
    const prior = service.getQuickInfoAtPosition(fileName, position);
    // Inside a string literal TypeScript has nothing to say, which is exactly
    // where our queries live. Where it does have something, it is about real
    // TypeScript and it wins.
    if (prior) return prior;
    try {
      const entry = analysisOf(fileName);
      if (!entry || entry.response.queries.length === 0) return prior;

      const offset = utf16ToByte(entry.text)(position);
      const inside = entry.response.queries.some(
        (query) => offset >= query.start && offset <= query.end,
      );
      if (!inside) return prior;

      const engine = ready();
      if (!engine) return prior;
      const hover = engine.analyzeHost({
        schema: findProject(fileName)?.schema ?? "",
        file_name: fileName,
        source: entry.text,
        hover: offset,
      }).hover;
      if (!hover) return prior;

      const start = entry.toUtf16(hover.start);
      return {
        kind: ts.ScriptElementKind.unknown,
        kindModifiers: "",
        textSpan: { start, length: Math.max(1, entry.toUtf16(hover.end) - start) },
        displayParts: [],
        documentation: [{ kind: "text", text: hover.markdown }],
        tags: [],
      };
    } catch (error) {
      log(`getQuickInfoAtPosition(${fileName}) failed: ${String(error)}`);
      return prior;
    }
  };

  return proxy;
}

/**
 * Merges two ascending `[start, length, classification]` triple streams.
 *
 * Clients build semantic tokens by walking these in order and pushing deltas;
 * one out-of-order triple makes every token after it land somewhere else.
 * TypeScript's own spans are ordered and so are ours, so this is a merge rather
 * than a sort.
 */
export function mergeSpans(left: readonly number[], right: readonly number[]): number[] {
  const merged: number[] = [];
  let l = 0;
  let r = 0;
  while (l < left.length && r < right.length) {
    if ((left[l] as number) <= (right[r] as number)) {
      merged.push(left[l] as number, left[l + 1] as number, left[l + 2] as number);
      l += 3;
    } else {
      merged.push(right[r] as number, right[r + 1] as number, right[r + 2] as number);
      r += 3;
    }
  }
  for (; l < left.length; l += 3) {
    merged.push(left[l] as number, left[l + 1] as number, left[l + 2] as number);
  }
  for (; r < right.length; r += 3) {
    merged.push(right[r] as number, right[r + 1] as number, right[r + 2] as number);
  }
  return merged;
}
