# @surrealguard/ts-plugin

SurrealQL diagnostics, highlighting and hovers inside `db.query("…")` — reported
as **TypeScript's own answers**, by a TypeScript language service plugin.

```jsonc
// tsconfig.json
{
  "compilerOptions": {
    "plugins": [{ "name": "@surrealguard/ts-plugin" }]
  }
}
```

```sh
pnpm add -D @surrealguard/ts-plugin
```

That is the whole setup. The plugin finds your `surrealguard.toml` by walking up
from each source file, reads your schema, and answers about the queries it finds
in that file. **Nothing happens in a project without a `surrealguard.toml`** —
not even the analyzer being loaded.

VS Code users additionally need `"typescript.tsserver.pluginPaths"` to be
unnecessary — it is, for a locally installed plugin — but do need the workspace
TypeScript version selected (`TypeScript: Select TypeScript Version` → *Use
Workspace Version*) if the plugin is a workspace dependency.

## Why a plugin and not a language server

SurrealGuard already ships `surrealguard-lsp`, and it works. But as a *second*
language server it competes with TypeScript for the same byte ranges: its
semantic tokens are overlaid on TypeScript's for the same string literal, and
the editor resolves that differently on every keystroke. The query flickers
between highlighted and plain-string green.

A plugin removes the conflict by construction. It proxies the language service,
so our diagnostics and classifications *are* TypeScript's — there is nothing to
merge and no race. It also reaches every TypeScript-aware editor rather than the
ones with a SurrealGuard extension.

## What it does

| Language service method | What the plugin adds |
| --- | --- |
| `getSemanticDiagnostics` | every finding on every embedded query, at its exact span inside the string |
| `getEncodedSemanticClassifications` | token kinds inside the query, so it stops being one flat string |
| `getQuickInfoAtPosition` | the field/parameter type under the cursor, when TypeScript has nothing to say there |

Every other method passes straight through.

Diagnostics carry `source: "surrealguard"` and a numeric code of
`1_000_000 + the finding number` — `E1002` is `1001002`, `L7014` is `1007014`.
TypeScript's own codes are five digits at most, so the two can never collide and
you can filter on ours. The finding's own code is repeated at the end of the
message, because a seven-digit number is not something to look up.

Severity maps to TypeScript's three: errors are errors, warnings are warnings,
and lint-level findings are **suggestions** — the faint underline, not a red
squiggle in code that runs.

### Highlighting is partial, on purpose

TypeScript has two classification vocabularies and the newer one is smaller.

- The **Original** format has `keyword`, `comment`, `string`, `number`,
  `operator`, `regexp`. Everything SurrealQL has, it can express, and the plugin
  emits all of it.
- The **2020** format — the one VS Code's TypeScript extension asks for — has
  twelve purely *semantic* kinds and no lexical ones at all. There is no
  encoding for "this word is a keyword", so `SELECT` inside a query keeps
  whatever colour the string literal already had.

In the 2020 format the plugin therefore classifies the identifier half only:
tables, fields, `$params`, `fn::` calls and type names. Painting `SELECT` as,
say, a namespace to force *some* colour would give it whatever the user picked
for TypeScript namespaces — a wrong answer where the current one is merely
uninformative. What it does fix in both formats is the flicker, which was the
actual complaint.

## What it does not do

- **`tsc` never loads plugins.** That is TypeScript's design, and it is the
  right split: keep `surrealguard check` in CI, where it sees the whole
  workspace at once instead of one file at a time.
- **`.svelte` / `.vue` files are not covered yet.** Not because of extraction —
  the analyzer reads their `<script>` blocks fine — but because the tools that
  own those files build their TypeScript language service with
  `ts.createLanguageService(…)` directly and never read `compilerOptions.plugins`.
  `svelte-check` was checked and does not load the plugin. Until that changes,
  those files need `surrealguard-lsp` or `surrealguard check`.
- **A `surrealguard.toml` with an exotic `[sources]` table** may resolve to the
  default globs here. The plugin reads `schema` / `queries` / `ignore` as string
  arrays and falls back to the defaults for anything else, rather than carrying a
  TOML parser into tsserver. `surrealguard check` remains the authority.

## How it works

The analyzer is the same one the browser playground runs: a `wasm32-wasip1`
build of the Rust workspace, instantiated in-process from
`wasm/surrealguard.wasm`. The alternative — spawning `surrealguard check` per
keystroke — costs a process, a pipe and a schema re-read every time. In-process
WASM makes it a function call, keeps the parsed schema catalog warm across
keystrokes, and means installing this package installs the analyzer.

One call answers all three questions for a file, so the second language-service
method of a keystroke is free. The cache is keyed on `(file text, schema
version)`; the schema is re-`stat`ed at most twice a second.

Measured on an M-series Mac, a 200-line file with 40 queries against a
22-statement schema:

| | |
| --- | --- |
| WASM compile + instantiate | 7 ms, once per tsserver process |
| analysis, 40 queries | 1.8 ms (p95 3.7 ms) |
| analysis, 2 queries | 0.10 ms |
| analysis, schema also changed | 2.1 ms |
| project lookup, warm | ~0 ms |
| repeat request, unchanged file | 0 ms — served from cache |

## License

MIT OR Apache-2.0.
