/**
 * Finding the project a source file belongs to, and its schema.
 *
 * The rule is `surrealguard check`'s: walk up from the file's directory until
 * a `surrealguard.toml` turns up. **No config means no project**, and no
 * project means the plugin says nothing at all — not "unknown table" on every
 * query, which is what analysing against an empty schema would produce. A
 * plugin installed in a repo that does not use SurrealGuard has to be
 * invisible, and the config file is the only signal that someone opted in.
 *
 * Everything here is cached, because it is on the keystroke path:
 *
 * - the upward walk, per directory, forever (a `surrealguard.toml` appearing
 *   mid-session is a project reload in every editor anyway);
 * - the schema text, per root, revalidated by mtime and no more often than
 *   {@link REVALIDATE_MS} — a `stat` per `.surql` file per keystroke is real
 *   cost for a change that happens when someone saves a migration.
 */

import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";

/** How often, at most, the schema files are re-`stat`ed. */
const REVALIDATE_MS = 500;

/** The `[sources]` globs, with the same defaults the Rust config has. */
interface Sources {
  schema: string[];
  queries: string[];
  ignore: string[];
}

const DEFAULT_SOURCES: Sources = {
  schema: ["**/*.surql", "**/*.surrealql"],
  queries: ["**/*.surql", "**/*.surrealql"],
  ignore: ["target/**", "node_modules/**", ".git/**"],
};

/** A resolved project: where it is, and what its schema currently says. */
export interface Project {
  /** The directory holding `surrealguard.toml`. */
  root: string;
  /** Every `.surql` source concatenated, schema-glob matches first. */
  schema: string;
  /**
   * Changes whenever {@link Project.schema} does. Callers key their own caches
   * on it so a schema edit invalidates them without comparing kilobytes of
   * text.
   */
  version: number;
}

interface Cached {
  sources: Sources;
  configMtime: number;
  schema: string;
  version: number;
  /** The `(path, mtime)` set the schema was built from. */
  stamp: string;
  checkedAt: number;
}

const roots = new Map<string, string | null>();
const projects = new Map<string, Cached>();
let versionCounter = 0;

/**
 * The directory containing the `surrealguard.toml` that governs `fileName`, or
 * `null` when there is none above it.
 */
export function findRoot(fileName: string): string | null {
  let dir = dirname(fileName);
  const visited: string[] = [];
  for (;;) {
    const known = roots.get(dir);
    if (known !== undefined) {
      for (const seen of visited) roots.set(seen, known);
      return known;
    }
    visited.push(dir);
    if (exists(join(dir, "surrealguard.toml"))) {
      for (const seen of visited) roots.set(seen, dir);
      return dir;
    }
    const parent = dirname(dir);
    if (parent === dir) {
      for (const seen of visited) roots.set(seen, null);
      return null;
    }
    dir = parent;
  }
}

/**
 * The project governing `fileName`, or `null` when the file is not in one.
 *
 * Revalidates the schema at most every {@link REVALIDATE_MS}; between those
 * points the answer is the cached text, which is the whole reason a keystroke
 * costs one analysis rather than a directory walk plus one analysis.
 */
export function findProject(fileName: string): Project | null {
  const root = findRoot(fileName);
  if (root === null) return null;

  const now = Date.now();
  const cached = projects.get(root);
  if (cached && now - cached.checkedAt < REVALIDATE_MS) {
    return { root, schema: cached.schema, version: cached.version };
  }

  const configMtime = mtime(join(root, "surrealguard.toml"));
  const sources =
    cached && cached.configMtime === configMtime
      ? cached.sources
      : readSources(join(root, "surrealguard.toml"));

  const files = discover(root, sources);
  const stamp = files.map((file) => `${file}:${mtime(join(root, file))}`).join("\n");
  if (cached && cached.stamp === stamp && cached.configMtime === configMtime) {
    cached.checkedAt = now;
    return { root, schema: cached.schema, version: cached.version };
  }

  const schema = files
    .map((file) => read(join(root, file)))
    .filter((text) => text.length > 0)
    // A newline between sources so the last statement of one cannot run into
    // the first of the next.
    .join("\n");
  const version = ++versionCounter;
  projects.set(root, { sources, configMtime, schema, version, stamp, checkedAt: now });
  return { root, schema, version };
}

/** Forgets everything cached. Tests call it; the plugin never does. */
export function resetProjectCache(): void {
  roots.clear();
  projects.clear();
}

/**
 * The `.surql` sources under `root`, **schema-glob matches first**.
 *
 * Order is load-bearing: the analyzer accumulates catalog effects in statement
 * order, so a `DEFINE` has to be seen before the query that reads it. A file
 * matching both globs counts as schema, because analysing a definition early
 * is always safe. This mirrors `discover_surrealql_sources` in the CLI.
 */
function discover(root: string, sources: Sources): string[] {
  const found: string[] = [];
  walk(root, root, sources.ignore, found);
  found.sort();
  const schema = found.filter((file) => matchesAny(file, sources.schema));
  const queries = found.filter((file) => !matchesAny(file, sources.schema));
  return [...schema, ...queries];
}

function walk(root: string, dir: string, ignore: string[], found: string[]): void {
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    const full = join(dir, entry.name);
    const rel = toPosix(relative(root, full));
    if (isIgnored(rel, ignore)) continue;
    if (entry.isDirectory()) {
      walk(root, full, ignore, found);
    } else if (entry.name.endsWith(".surql") || entry.name.endsWith(".surrealql")) {
      found.push(rel);
    }
  }
}

/**
 * Mirrors the CLI's `matches_simple_ignore`: a pattern is a directory name
 * (with any `/**` suffix dropped) and matches when any path component equals
 * it. Deliberately the same approximation, so the editor and CI skip the same
 * directories.
 */
function isIgnored(rel: string, ignore: string[]): boolean {
  const components = rel.split("/");
  return ignore.some((pattern) => {
    const trimmed = pattern.endsWith("/**") ? pattern.slice(0, -3) : pattern;
    return components.includes(trimmed);
  });
}

function matchesAny(rel: string, globs: string[]): boolean {
  return globs.some((glob) => globToRegExp(glob).test(rel));
}

const regexpCache = new Map<string, RegExp>();

/**
 * A glob as an anchored regular expression: `**` crosses directory
 * separators, `*` and `?` do not.
 */
function globToRegExp(glob: string): RegExp {
  const cached = regexpCache.get(glob);
  if (cached) return cached;

  let pattern = "";
  for (let i = 0; i < glob.length; i++) {
    const char = glob[i] as string;
    if (char === "*") {
      if (glob[i + 1] === "*") {
        // `**/` may match nothing at all, so `**/*.surql` covers `a.surql`.
        if (glob[i + 2] === "/") {
          pattern += "(?:[^/]*/)*";
          i += 2;
        } else {
          pattern += ".*";
          i += 1;
        }
      } else {
        pattern += "[^/]*";
      }
    } else if (char === "?") {
      pattern += "[^/]";
    } else {
      pattern += char.replace(/[.+^${}()|[\]\\]/g, "\\$&");
    }
  }
  const regexp = new RegExp(`^${pattern}$`);
  regexpCache.set(glob, regexp);
  return regexp;
}

/**
 * The `[sources]` globs from a `surrealguard.toml`.
 *
 * A deliberately small reader rather than a TOML dependency: the plugin is
 * loaded into tsserver, where every dependency is another thing that can fail
 * to resolve, and the three keys it needs are all plain string arrays. Any key
 * it cannot read falls back to the default — the same result as not setting it,
 * which is what the overwhelming majority of configs do. A `surrealguard.toml`
 * with an exotic `[sources]` gets the default globs in the editor and the real
 * ones in `surrealguard check`; that is a narrower discrepancy than refusing to
 * load, and `check` remains the authority.
 */
function readSources(configPath: string): Sources {
  const text = read(configPath);
  if (!text) return DEFAULT_SOURCES;

  const table = tableBody(text, "sources");
  if (table === null) return DEFAULT_SOURCES;

  return {
    schema: stringArray(table, "schema") ?? DEFAULT_SOURCES.schema,
    queries: stringArray(table, "queries") ?? DEFAULT_SOURCES.queries,
    ignore: stringArray(table, "ignore") ?? DEFAULT_SOURCES.ignore,
  };
}

/** The lines of `[name]` up to the next table header, or `null`. */
function tableBody(text: string, name: string): string | null {
  const lines = text.split(/\r?\n/);
  const start = lines.findIndex((line) => line.trim() === `[${name}]`);
  if (start < 0) return null;
  const rest = lines.slice(start + 1);
  const end = rest.findIndex((line) => /^\s*\[/.test(line));
  return (end < 0 ? rest : rest.slice(0, end)).join("\n");
}

/** `key = ["a", "b"]` or `key = "a"`, single- or multi-line. */
function stringArray(body: string, key: string): string[] | null {
  const at = body.match(new RegExp(`^\\s*${key}\\s*=\\s*`, "m"));
  if (!at || at.index === undefined) return null;
  const rest = body.slice(at.index + at[0].length);
  if (rest.startsWith("[")) {
    const close = rest.indexOf("]");
    if (close < 0) return null;
    const items = rest.slice(1, close).matchAll(/"([^"]*)"|'([^']*)'/g);
    return [...items].map((item) => item[1] ?? item[2] ?? "");
  }
  const single = rest.match(/^"([^"]*)"|^'([^']*)'/);
  return single ? [single[1] ?? single[2] ?? ""] : null;
}

function exists(path: string): boolean {
  try {
    statSync(path);
    return true;
  } catch {
    return false;
  }
}

function mtime(path: string): number {
  try {
    return statSync(path).mtimeMs;
  } catch {
    return -1;
  }
}

function read(path: string): string {
  try {
    return readFileSync(path, "utf8");
  } catch {
    return "";
  }
}

function toPosix(path: string): string {
  return sep === "/" ? path : path.split(sep).join("/");
}
