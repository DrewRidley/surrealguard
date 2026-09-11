/**
 * `surrealqlAnalyzer()` — the Svelte preprocessor that makes an inline query
 * attribute mean what it looks like it means.
 *
 * ```js
 * // svelte.config.js
 * import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";
 * import { surrealqlAnalyzer } from "@surrealdb/analyzer-svelte/preprocess";
 *
 * export default { preprocess: [surrealqlAnalyzer(), vitePreprocess()] };
 * ```
 *
 * ```svelte
 * <Query q="SELECT id, name FROM person WHERE age > {minAge}">
 * ```
 *
 * **Without this, that attribute is a bug.** Svelte compiles an interpolated
 * attribute to string concatenation, so what `<Query>` would receive is a
 * finished string with the value spliced into it: a SurrealQL injection for a
 * string value, and — because the text changes on every keystroke — a fresh
 * cache entry per frame for a numeric one. The parts have to be captured
 * *before* concatenation, which is only possible ahead of the compiler.
 *
 * So this rewrites, on `<Query>` and `<LiveQuery>` only:
 *
 * ```svelte
 * <Query q="SELECT id, name FROM person WHERE age > {minAge}">
 * ```
 * ```svelte
 * <Query q={() => __sg_query(["SELECT id, name FROM person WHERE age > ", ""], [minAge])}>
 * ```
 *
 * which yields the text `SELECT id, name FROM person WHERE age > $__host0` and
 * the parameters `{ __host0: minAge }` — see `./inline.js`, which owns that
 * naming and is the only place it is spelled.
 *
 * The thunk is not decoration either: it is what makes the attribute reactive.
 * `Source<Q>` resolves a thunk inside a tracking context, so `minAge` is read
 * there, and moving it re-runs the query. Without it the value would be read
 * once, at construction, which is the exact defect the `Source` design exists
 * to close.
 *
 * What is left alone:
 *
 * - `q={someQuery}` — an expression attribute is already a query value.
 * - `q` on any other element or component. Only `Query` and `LiveQuery`.
 * - Every other attribute, and every other file.
 *
 * A plain `q="SELECT id, name FROM person"` with no interpolation goes through
 * the same path, with no parameters, so one form covers both.
 */

import MagicString from "magic-string";
import { parse } from "svelte/compiler";

/** The names this rewrites. Matched on the tag, so an alias is not covered. */
const QUERY_COMPONENTS = new Map([
  ["Query", "__sg_query"],
  ["LiveQuery", "__sg_live"],
]);

const IMPORT_STATEMENT =
  'import { sgQuery as __sg_query, sgLive as __sg_live } from "@surrealdb/analyzer-svelte/inline";';

/** Minimal shapes from Svelte's modern AST — enough to walk what we care about. */
interface Node {
  type: string;
  start: number;
  end: number;
  [key: string]: unknown;
}

interface AttributeValueNode extends Node {
  raw?: string;
  data?: string;
}

export interface PreprocessorOutput {
  code: string;
  map?: unknown;
}

export interface MarkupPreprocessor {
  markup(input: { content: string; filename?: string }): PreprocessorOutput | undefined;
}

export interface SurrealQLAnalyzerPreprocessOptions {
  /**
   * Which tags carry an inline query. Defaults to `Query` and `LiveQuery`.
   * Supply this if you re-export them under other names.
   */
  components?: Record<string, "query" | "live">;
}

export function surrealqlAnalyzer(options: SurrealQLAnalyzerPreprocessOptions = {}): MarkupPreprocessor {
  const components = new Map(QUERY_COMPONENTS);
  for (const [name, kind] of Object.entries(options.components ?? {})) {
    components.set(name, kind === "live" ? "__sg_live" : "__sg_query");
  }

  return {
    markup({ content, filename }) {
      // Cheap bail-out, so a project's other few hundred components pay
      // nothing: neither tag can appear without its name appearing.
      if (!/<(Query|LiveQuery)\b/.test(content) && !hasCustom(content, components)) {
        return undefined;
      }

      let ast;
      try {
        ast = parse(content, { modern: true, filename }) as unknown as {
          fragment: Node;
          instance?: { content: { start: number } };
          module?: { content: { start: number } };
        };
      } catch {
        // A file mid-edit does not parse. Leave it exactly as it is and let the
        // compiler report the syntax error, which it does far better than this
        // could.
        return undefined;
      }

      const source = new MagicString(content);
      let rewrote = false;

      walk(ast.fragment, (node) => {
        if (node.type !== "Component") return;
        const helper = components.get(String(node.name));
        if (!helper) return;
        const attributes = node.attributes as Node[] | undefined;
        const attribute = attributes?.find(
          (candidate) => candidate.type === "Attribute" && candidate.name === "q",
        );
        if (!attribute) return;
        const parts = attribute.value;
        // `q` (boolean) and `q={expr}` (a single ExpressionTag, which Svelte
        // does not wrap in an array) are already-valid forms. Only the quoted
        // form — which Svelte represents as a list of Text / ExpressionTag —
        // is ours.
        if (!Array.isArray(parts)) return;

        const rewritten = rewrite(parts as AttributeValueNode[], content, helper);
        if (!rewritten) return;
        source.overwrite(attribute.start, attribute.end, rewritten);
        rewrote = true;
      });

      if (!rewrote) return undefined;

      // The import goes just inside the instance `<script>`, so `lang="ts"`
      // still applies to it and a later TypeScript preprocessor sees it.
      const scriptStart = ast.instance?.content.start;
      if (scriptStart === undefined) {
        source.prepend(`<script>${IMPORT_STATEMENT}</script>\n`);
      } else {
        source.appendLeft(scriptStart, `\n${IMPORT_STATEMENT}`);
      }

      return {
        code: source.toString(),
        map: source.generateMap({ hires: true, source: filename }),
      };
    },
  };
}

function hasCustom(content: string, components: Map<string, string>): boolean {
  for (const name of components.keys()) {
    if (name !== "Query" && name !== "LiveQuery" && content.includes(`<${name}`)) return true;
  }
  return false;
}

/**
 * Turn the attribute's parts into `q={() => helper([...strings], [...values])}`.
 *
 * The strings array is emitted with one more element than the values array —
 * the template-literal shape — so the skeleton is unambiguous even when an
 * interpolation is the very first or very last thing in the attribute.
 */
function rewrite(
  parts: AttributeValueNode[],
  content: string,
  helper: string,
): string | undefined {
  const strings: string[] = [];
  const values: string[] = [];
  let pending = "";

  for (const part of parts) {
    if (part.type === "Text") {
      // `raw` is the source spelling, so an entity stays an entity; `data` is
      // the decoded text, and it is the decoded text that has to reach the
      // database.
      pending += String(part.data ?? part.raw ?? "");
      continue;
    }
    if (part.type !== "ExpressionTag") return undefined;
    strings.push(pending);
    pending = "";
    // Slice the expression out of the source rather than printing the AST
    // back: the source is what the author wrote, and it needs no
    // pretty-printer to stay correct.
    values.push(content.slice(part.start + 1, part.end - 1).trim());
  }
  strings.push(pending);

  return `q={() => ${helper}([${strings.map(quote).join(", ")}], [${values.join(", ")}])}`;
}

/** JSON is a subset of JS string syntax, so this is exactly right and short. */
function quote(value: string): string {
  return JSON.stringify(value);
}

/** Depth-first walk over `fragment.nodes`, plus every node's own fragment. */
function walk(node: Node | undefined, visit: (node: Node) => void): void {
  if (!node || typeof node !== "object") return;
  visit(node);
  const children = (node.nodes ?? (node.fragment as Node | undefined)?.nodes) as
    | Node[]
    | undefined;
  for (const child of children ?? []) walk(child, visit);
  // `{#if}` / `{#each}` / `{#await}` hold their branches on named properties
  // rather than in `nodes`, and a `<Query>` inside an `{#each}` is the whole
  // point, so those are walked too.
  for (const key of ["consequent", "alternate", "body", "fallback", "pending", "then", "catch"]) {
    const branch = node[key] as Node | undefined;
    if (branch && typeof branch === "object" && "type" in branch) walk(branch, visit);
  }
}
