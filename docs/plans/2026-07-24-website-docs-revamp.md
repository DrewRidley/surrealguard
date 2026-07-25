# Website + docs revamp (post-release) — 2026-07-24

Runs AFTER the crates.io 0.3.0 publish. Sequence the user set: publish → this revamp.

## 1. WASM update (must-do)
The live "Guard" hero + playground on surrealguard.dev run a WASM build compiled
from the analyzer (`web`/wasm crate). Rebuild + redeploy (Wrangler) so the site
demonstrates the *latest* engine: record-link traversal, `.{}` destructure typing
+ validation, sharper `Kind::Any`, inferred UDF returns, cross-source UDF return
resolution, multi-statement tuples, graceful hover degradation, exit-set block
types. None of this shows on the site until the WASM is rebuilt.

## 2. Install section overhaul
- Buttons/icons for **npm**, **crates.io**, **GitHub** (Zed = GitHub icon, not a
  "recommend the LSP" line — per prior feedback link to the Zed extension repo).
- npx as primary; list the newly-published crates (surrealguard, -rs, -macros,
  -lsp) + `cargo add surrealguard-rs` for the Rust SDK.
- Link crates.io + docs.rs (thorough docs shipped with the release).

## 3. Diagnostics docs — completeness + UI overhaul
- **Completeness:** docs must be consistent with EVERY diagnostic in the catalog
  (`crates/diagnostics/src/catalog.rs`) — one entry per code, with default level
  (allow/warn/deny), what triggers it, and a fix. No code undocumented; no
  documented code that doesn't exist. Include the graph edge-filter WHERE checks
  (`->(likes WHERE …)` / `->likes[WHERE …]`, validated against the filtered
  table — currently IMPLEMENTED but absent from DESIGN.md/docs).
- **UI overhaul:** the codes/lints table needs more color + a landing-page feel
  (not a bland table) — grouped by family + level, severity color-coding, inline
  `allow(reason=)` suppression documented. Fold in all prior styling feedback (no
  generic AI gradients; unique/real).

## 4. AI section — agents using SurrealGuard
Headline (works today, unambiguous): agents (Claude Code, etc.) run
`surrealguard check` / `surrealguard generate` in their loop — a compile-time-
style correctness signal for SurrealQL (unknown tables/fields, type mismatches)
+ typed query results, like `tsc`/`cargo check`. Secondary: editor/LSP + any
MCP-LSP bridge. VERIFY Claude Code's exact LSP-consumption story (native vs MCP
vs CLI) before writing — do not overclaim.

## 5. Multi-language embedded checking (feature, not just docs)
Today embedded SurrealQL extraction = TS-family (`ts/tsx/js/jsx`) + framework
`<script>` (`svelte/vue/astro/html`) + Rust (macros). Extend to more host langs
(Python, Go, PHP, Java, C#, Ruby) via each language's tree-sitter grammar + that
SDK's query sink (e.g. Python `db.query("…")`, Go `db.Query(ctx, "…")`), mirroring
`crates/embed/src/typescript.rs`. Plain `.surql` is already checked natively.

## LSP polish (code, separate from website — do with the review pass or standalone)
- **Inferred return-type ghost** (rust-analyzer style): for a `DEFINE FUNCTION`
  with NO explicit `-> T`, emit a grey `InlayHintKind::TYPE` right after the
  params `)` showing the inferred return: ` -> <int>`, ` -> <record<...>>`, or
  ` -> none` for a unit/none body. Suppress when `-> T` is written. Source =
  `FunctionDef.inferred_return` (now correct cross-source). Aesthetic decision:
  render the none case as SurrealQL `none` (consistent) vs the user's liked `()`
  — default `none`, confirm.

## Accumulated user feedback to honor
- No generic/AI gradients; unique, not bland. Real WASM engine, nothing faked.
- Zed = link to the extension's GitHub, don't "recommend installing the LSP".
- Hover uses markdown + syntax-highlighted `surql` fences + colors (shipped).
- Inlay hints are cast-form `<T>`, not `: T` (shipped).
