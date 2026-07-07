# Typed AST lowering

Status: **implemented** (2026-07-04). All seven migration steps landed for the
type-inference axis; see "Completion status" at the end for the precise
remaining old-engine inventory (frozen validators awaiting the diagnostics
phase). Historical design notes below are kept as written; `[v2]`/`[v3]`
markers show review-pass changes.

## Problem

Analyzers currently consume raw tree-sitter `Node`s. Tree-sitter gives us spans,
error recovery, and incrementality — but a CST is a *parse* artifact, not an
*analysis* input. Every analyzer re-derives structure from it by hand, and that
plumbing is where the codebase's fragility and unreadability live. Concrete
evidence, all in tree today:

- **Double parsing.** `select_ir.rs::field_path_from_node` flattens a projection
  like `->friend->user.{name, age}` into a `String`, then
  `select.rs::graph_lookups_from_projection_text` re-parses that string
  character-by-character looking for `->`/`<-`, and
  `brace_selector_from_text` re-parses `{name, age}` by splitting on commas.
  The grammar already produced `Lookup`, `LookupSelection`, `GraphFieldSelection`,
  and `Destructure` nodes for all of this; we discarded them and wrote a second,
  worse parser downstream.
- **Substring classification.** `mutation.rs::mutation_return_mode` classifies a
  RETURN clause by `.contains("NONE")` on uppercased source text.
  `RETURN nonexistent_field` contains `"NONE"`; `RETURN difference` contains
  `"DIFF"`. These are real misclassifications waiting to happen.
- **Hand-written keyword scanners.** `semantic.rs::leading_table_references` and
  `table_references_after_keyword` walk children, uppercase text, and look for
  statement keywords — parsing, again, on top of the parse tree.
- **Helper duplication.** `node_text`, `node_span`, `is_identifier_like`,
  `table_name_from_node_text` are copy-pasted across at least four modules.
- **The half-measure that proves the point.** `SelectIr` is a typed AST for
  SELECT — invented because raw CST walking was unbearable for the hardest
  statement — but it is lossy (paths become text), SELECT-only, and lives in
  the analysis crate.
- **Type syntax treated as opaque text.** `DEFINE FIELD tags ON person TYPE
  array<string>` currently produces `PartialReason::UnsupportedSyntax("array<string>")`
  because schema extraction parses type text ad hoc. The grammar has a complete
  structured type family (`Type`, `ArrayType`, `UnionType`, `ParameterizedType`,
  `LiteralType`, `ObjectType`) that we ignore.

Surrealix (the predecessor) was readable precisely because its analyzers matched
on typed structures (`Part::Graph { dir, what }`). That readability came from
having a typed AST — not from `surrealdb::sql`'s parser, whose lack of spans,
lack of error recovery, and semver-unstable internals are why it was abandoned.
We can have the ergonomics without the foundation.

## Proposal

Introduce one lowering pass and a typed, span-carrying AST between the CST and
the analyzers:

```text
source text ──tree-sitter──▶ CST ──lower (once)──▶ ast::Statement ──▶ analyzers
                                      │
                                      └── ALL node-walking, keyword scanning,
                                          case folding, grammar-quirk handling,
                                          and normalization lives here
```

Analyzers consume only `ast::*` types. No analyzer touches `tree_sitter::Node`
or source text again.

### Goals

- Per-token spans preserved on every AST node (per `->`, per field, per alias) —
  the diagnostics phase depends on this.
- Partial/erroneous input lowers to explicit partial nodes; the "no hidden
  uncertainty" rule is enforced structurally at one boundary instead of by
  convention in ~40 analyzers.
- Analyzers become pure tree transforms: `SelectStmt -> Kind` (D7).
  Constructible in unit tests without parsing strings.
- One home for grammar quirks (`LimitStartComboClause`, `type::is::record` →
  `type::is_record`, keyword case folding).
- Per-statement analyzers keep owning their own logic; composite constructs
  (Block, IF/ELSE, FOR bodies) contain `Vec<Spanned<Statement>>` and their
  analyzers dispatch each child to that child's own analyzer — the AST makes
  the pass-through structural, so per-statement invariants attach to their own
  analyzer in the diagnostics phase. `[v2: made explicit — this is a standing
  requirement, not an accident of the sketch]`

### Non-goals

- No name resolution, schema lookup, or env in the AST. It is purely syntactic.
  (What compilers split into AST vs HIR: we are building the AST; the analyzers
  remain the HIR-equivalent layer.)
- No custom scalar/type hierarchy. Leaf kinds remain upstream
  `surrealdb_types::Kind`; the AST's `TypeExpr` is *syntax* that analyzers
  convert to `Kind`.
- No change to the analyzer tree layout (one file per construct, each owning its
  own logic), the function signature table, or the
  type-inference-before-diagnostics sequencing. `[v3: ResponseShape was
  originally listed here as untouched; D7 deletes it — response types are
  plain `Kind`.]`
- Not a general-purpose SurrealQL AST for third parties. Model what analysis
  consumes; leave the rest reachable via spans (see escape hatch).

## Design decisions

### D1. Owned lowered AST, not typed views over the CST

Two ways to give analyzers typed access:

a) **Typed views** (rust-analyzer's `ast` layer): zero-copy wrappers around
   `Node` with typed accessor methods. Always in sync with the tree, no
   duplication.
b) **Owned lowered AST**: one pass builds owned structs/enums; the CST is
   dropped or kept only for tooling.

Recommend **(b)**:

- Owned nodes are constructible in tests without parsing — analysis tests get
  dramatically simpler and stop depending on grammar details.
- Lowering can *normalize* (case folding, `LimitStartComboClause` flattening,
  function-path normalization) so quirks never reach analyzers. Views can't;
  every accessor re-handles the quirk.
- Views inherit `Node<'tree>` lifetimes, which poison every analyzer signature
  and forbid storing AST fragments (e.g. in `StatementEnv`).
- Cost: an extra allocation of the tree per parse. Files are small; lowering is
  linear and trivially cheap next to analysis. For LSP incrementality later,
  re-lowering a whole file per keystroke is fine (rust-analyzer re-lowers
  per-item; our files are single-digit KB).

### D2. Home: `crates/syntax`

`surrealguard-syntax` already owns text → CST (`parse.rs`, `source.rs`,
`span.rs`). Lowering is a syntax concern; it moves the crate's contract from
"you get a CST" to "you get an AST". Layout:

```text
crates/syntax/src/
  ast/
    mod.rs          // Spanned, PartialNode, common types
    statement.rs    // Statement enum + per-statement structs
    expr.rs         // Expr, Idiom, IdiomPart, Literal, Call
    ty.rs           // TypeExpr (syntactic types)
    clause.rs       // shared clauses (ReturnMode, DataClause, Order, ...)
  lower/
    mod.rs          // entry: lower(parsed: &ParsedSource) -> ast::Script
    ...             // one module per statement family, mirroring ast/
```

`surrealguard-workspace` then depends on `ast::*` instead of `tree_sitter::Node`.
The `tree-sitter` dependency disappears from the workspace crate entirely when
migration completes — a good doneness check.

Dependency note: `ast::TypeExpr` is syntactic, so `syntax` does *not* need
`surrealdb-types`. Conversion `TypeExpr -> Kind` lives in the workspace crate.

### D3. Spans on everything

```rust
pub struct Spanned<T> {
    pub node: T,
    pub span: ByteRange,       // existing span.rs type; SourceId travels separately
}
```

Every AST struct either is `Spanned<T>` or carries explicit span fields for its
sub-tokens where diagnostics will point. A graph step carries the span of its
direction token *and* its edge name separately (`dir: Spanned<GraphDir>` —
`[v2: v1's sketch claimed per-arrow spans but didn't carry one; fixed]`).
Spans are `Copy` byte ranges — no `Node` references, no lifetimes.

The escape hatch: because every node knows its byte range, anything the AST
doesn't model is still reachable by slicing source text or re-querying the CST.
Unmodeled constructs are never *lost*, just not yet convenient.

### D4. Partiality is a lowering output, not an analyzer convention

`[v2: reworked. v1 proposed a `MaybeLowered<T>` wrapper AND `PartialNode`
variants in the same sketch — two mechanisms for one concept. The wrapper is
gone; there is exactly one mechanism, in two placements.]`

```rust
pub struct PartialNode {
    pub span: ByteRange,
    pub cst_kind: String,      // "ERROR", "MISSING ...", or the unmodeled kind
}
```

**Placement 1 — enum variants.** Every AST enum whose position can fail to
lower gets a `Partial(PartialNode)` variant: `Statement::Partial`,
`Expr::Partial`, `Projection::Partial`, `Source::Partial`,
`GraphTarget::Partial`, `TypeExpr::Partial`. Exhaustive matching forces every
analyzer to handle it; uncertainty cannot be silently dropped. No wrapper type
appears in signatures.

**Placement 2 — the `unhandled` bucket.** Every statement struct carries:

```rust
pub unhandled: Vec<PartialNode>,   // CST children the lowering didn't consume
```

This exists because enum variants only cover *modeled positions*. The grammar
is permissive: `node-types.json` says a `SelectStatement` may directly contain
`ReturnClause`, `WithClause`, `VersionClause`, `TempfilesClause` — clauses a
fixed-field `SelectStmt` would otherwise silently swallow, violating the rule
this design exists to enforce. Lowering appends every unconsumed named child to
`unhandled`; analyzers translate a non-empty bucket into a partial response
shape (exactly what `advanced_select_partial_reason` does today for
`RETURN`-on-SELECT, but generalized and impossible to forget). `[v2: new —
found by checking the sketch against the real grammar inventory]`

Analyzers map `PartialNode` to today's `PartialReason` values; the
workspace-crate concept stays where it is and `syntax` stays decoupled from it.

### D5. Normalization happens during lowering

One place, once:

- keyword case folding (no more `.eq_ignore_ascii_case` scattered around)
- `LimitStartComboClause` flattened into separate limit/start
- function paths normalized (`type::is::record` → `type::is_record`)
- record-id table extraction (`person:one` → table `person`, id preserved
  with its own span)
- string-literal prefix detection (`d'...'`/`u'...'`/`r'...'` → typed literal
  variants; replaces byte-peeking in `expression.rs`)

### D6. Function analyzer signature `[v2: new — decided now to avoid migrating 373 signatures twice]`

The 373 built-in function analyzers currently take
`(ctx, node: Node, args: &[Kind])`. They become:

```rust
pub fn analyze_string_len(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind
```

Same arity, mechanical swap. `ast::Call` carries the normalized path span and
each argument's `Spanned<Expr>` — so when the diagnostics phase lands,
"argument 2 has type X, expected Y" can point at argument 2's own span without
a second signature migration. Nothing speculative is added: `args: &[Kind]`
stays the evaluation input; `call` is data that already exists.

### D7. Response types are `Kind`; uncertainty is emitted, not stored `[v3 — added after steps 2–3 landed]`

`ResponseShape`/`FieldShape` are deleted. A statement's inferred response
type is a plain upstream `surrealdb_types::Kind`:

- closed object with known fields → `Kind::Literal(KindLiteral::Object(BTreeMap<String, Kind>))`
- open/unknown-fields object → `Kind::Object`
- arrays/sets keep `Kind::Array`/`Kind::Set` (with their `Option<u64>` max length)
- unions → `Kind::Either` (merged via upstream `Kind::either`, which flattens
  and dedups)
- undeterminable → `Kind::Any` as a poison value

Rationale: an audit found `ResponseShape` had zero production consumers for
everything it added over `Kind` — `FieldShape.span` and
`materialized_by_fetch` were written and never read, `Unknown { reason }`
was never destructured outside tests, and `ExpressionFact` carried `kind`
and `shape` redundantly. Meanwhile `KindLiteral::Object` already expresses
closed objects, so `ResponseShape` violated this project's own
no-competing-type-hierarchy rule.

Uncertainty follows the compiler pattern (rustc's `TyErr`): the *type* is a
bare poison marker; the *reason* is emitted at the inference site — carried
on transient `ExpressionFact`s (span + `PartialReason`) and, in the
diagnostics phase, formalized as coded findings. Reasons never ride inside
the type structure. "No hidden uncertainty" means uncertainty is always
reported, not always stored.

`FETCH` stops being a metadata flag and becomes what it means: eager
substitution of `Kind::Record([t])` (or an array of it) with table `t`'s
object literal. Recursion is bounded because `FETCH` depth is explicit.

## AST sketch

Sketch, not final — exact shapes settle during implementation review.
Illustrative of granularity and style:

```rust
pub struct Script {
    pub statements: Vec<Spanned<Statement>>,          // source order preserved
}

pub enum Statement {
    Select(SelectStmt),
    Create(CreateStmt),
    Update(UpdateStmt),
    Upsert(UpsertStmt),
    Delete(DeleteStmt),
    Insert(InsertStmt),
    Relate(RelateStmt),
    Define(DefineStmt),
    Remove(RemoveStmt),
    Alter(AlterStmt),
    Let(LetStmt),
    Return(ReturnStmt),
    IfElse(IfElseStmt),
    For(ForStmt),
    Block(Block),
    LiveSelect(LiveSelectStmt),
    // Begin/Cancel/Commit/Break/Continue/Throw/Use/Info/Show/Sleep/Option/
    // Kill/Rebuild: unit-ish or thin structs — but see the table-ref note below
    Partial(PartialNode),
}

pub struct Block {
    pub statements: Vec<Spanned<Statement>>,
    pub unhandled: Vec<PartialNode>,
}
// The block analyzer iterates and dispatches each child to that child's own
// analyzer — composability for the per-statement invariants phase.

pub struct SelectStmt {
    pub only: bool,
    pub value: bool,                                  // SELECT VALUE
    pub projections: Vec<Projection>,
    pub from: Vec<Spanned<Source>>,
    pub omit: Vec<Spanned<Idiom>>,
    pub fetch: Vec<Spanned<Idiom>>,
    pub split: Vec<Spanned<Idiom>>,
    pub where_clause: Option<Spanned<Expr>>,
    pub group: Option<GroupClause>,
    pub order: Option<OrderClause>,
    pub limit: Option<Spanned<Expr>>,                 // literal-ness decided by analyzer
    pub start: Option<Spanned<Expr>>,
    pub explain: Option<ByteRange>,
    pub timeout: Option<Spanned<Expr>>,
    pub parallel: Option<ByteRange>,
    pub unhandled: Vec<PartialNode>,                  // WITH/VERSION/TEMPFILES/stray RETURN → here, for now
}

pub enum Projection {
    Wildcard(ByteRange),
    Expr {                                            // covers plain fields too:
        expr: Spanned<Expr>,                          //   a bare field is Expr::Idiom
        alias: Option<Spanned<String>>,
    },
    Partial(PartialNode),
}

pub enum Source {
    Table(Spanned<String>),
    RecordId { table: Spanned<String>, id: ByteRange },
    Idiom(Idiom),                                     // graph traversal FROM
    Param(Spanned<String>),
    Subquery(Box<Spanned<Statement>>),
    Partial(PartialNode),
}

/// The load-bearing type. Kills every `->`-scanning function in the tree.
pub struct Idiom {
    pub parts: Vec<Spanned<IdiomPart>>,
}

pub enum IdiomPart {
    Field(String),
    Index(i64),
    All,                                              // .*
    Graph {
        dir: Spanned<GraphDir>,                       // Out / In / Both — arrow token span
        target: Spanned<GraphTarget>,
    },
    Destructure(Vec<Spanned<Idiom>>),                 // .{name, age} — real nodes, not comma-split text
    Where(Box<Spanned<Expr>>),                        // [WHERE ...] filters
    Partial(PartialNode),
}

pub enum GraphTarget {
    Table(String),                                    // ->likes
    Filtered {                                        // ->(likes WHERE since > $x)
        table: Spanned<String>,
        where_clause: Box<Spanned<Expr>>,
    },
    Partial(PartialNode),
}

pub struct Call {
    pub path: Spanned<String>,                        // pre-normalized
    pub args: Vec<Spanned<Expr>>,
}

pub enum Expr {
    Literal(Literal),                                 // Int/Float/Decimal/Strand/Bool/
                                                      // None/Null/Duration/Datetime/Uuid/Regex
    Idiom(Idiom),
    Param(Spanned<String>),
    Binary { lhs: Box<Spanned<Expr>>, op: Spanned<BinaryOp>, rhs: Box<Spanned<Expr>> },
    Prefix { op: Spanned<PrefixOp>, expr: Box<Spanned<Expr>> },
    Call(Call),
    Object(Vec<(Spanned<String>, Spanned<Expr>)>),
    Array(Vec<Spanned<Expr>>),
    Subquery(Box<Spanned<Statement>>),
    Block(Block),
    Cast { ty: Spanned<TypeExpr>, expr: Box<Spanned<Expr>> },
    Closure(PartialNode),                             // explicitly unmodeled for now
    Partial(PartialNode),
}

/// Replaces `.contains("NONE")` with a parse.
pub enum ReturnMode {
    None(ByteRange),
    Diff(ByteRange),
    Before(ByteRange),
    After(ByteRange),
    Fields(Vec<Projection>),
}

pub struct CreateStmt {
    pub targets: Vec<Spanned<Source>>,
    pub data: Option<DataClause>,                     // SET/UNSET/CONTENT/MERGE/PATCH/REPLACE
    pub ret: Option<Spanned<ReturnMode>>,
    pub unhandled: Vec<PartialNode>,
    // timeout/parallel/version as in SelectStmt
}
// UpdateStmt/UpsertStmt/DeleteStmt follow CreateStmt's shape.
// RelateStmt models `from -> edge -> to` as three spanned positions explicitly.

/// [v2: INSERT is NOT analogous — the grammar has BulkInsert and FieldAssignment
/// children the others lack. It models its payload forms distinctly:]
pub struct InsertStmt {
    pub target: Option<Spanned<Source>>,              // INTO <target>
    pub data: InsertData,
    pub ret: Option<Spanned<ReturnMode>>,
    pub unhandled: Vec<PartialNode>,
}

pub enum InsertData {
    Values(Vec<Spanned<Expr>>),                       // object or array-of-objects payload
    Tuples {                                          // INSERT INTO t (a, b) VALUES (...), (...)
        columns: Vec<Spanned<String>>,
        rows: Vec<Vec<Spanned<Expr>>>,
    },
    Assignments(Vec<(Spanned<Idiom>, Spanned<Expr>)>),// INSERT ... SET-style FieldAssignments
    Partial(PartialNode),
}

/// Syntactic types — finally makes `array<string>` a modeled construct.
pub enum TypeExpr {
    Name(Spanned<String>),                            // string, int, record, ...
    Parameterized { name: Spanned<String>, args: Vec<Spanned<TypeExpr>> },
    Union(Vec<Spanned<TypeExpr>>),
    Optional(Box<Spanned<TypeExpr>>),
    Partial(PartialNode),                             // literal types, object types: later
}
```

**Thin statements still carry their table references.** `[v2: new — verified
against `semantic.rs::collect_table_reference_diagnostics`, which resolves
table refs for LIVE SELECT, ALTER, REBUILD, SHOW, and INFO FOR in addition to
the mutations.]` `LiveSelectStmt`, `AlterStmt`, `RebuildStmt`, `ShowStmt`, and
`InfoStmt` are thin, but each carries its `Option<Spanned<String>>` table
position — otherwise the keyword scanners cannot actually be deleted in step 7
and this doc's "what dies" list is false.

Coverage is tiered. Tier 1 (fully modeled): everything type inference consumes
today — SELECT, the six mutations, LET/RETURN/IF/FOR/Block, expressions, idioms,
DEFINE TABLE/FIELD/INDEX/EVENT/PARAM/FUNCTION/ANALYZER, REMOVE/ALTER, TypeExpr,
plus the thin-statement table refs above. Tier 2 (`unhandled`/`Partial`
initially): the long tail of DEFINE variants (ACCESS/API/BUCKET/CONFIG/analyzer
tuning clauses like `Bm25Clause`/`HnswClause`/...) — modeled on demand when an
analyzer needs them, never silently dropped (D4). Of the 217 named grammar
kinds, roughly half are Tier 1.

## What this fixes, what dies, what survives

Dies (deleted outright once migration completes):

- `select_ir.rs` — absorbed by `ast::SelectStmt` (its IR was this idea, lossy)
- `graph_lookups_from_projection_text`, `brace_selector_from_text`,
  `row_brace_selector_parts`, `graph_projection_parts` — replaced by `IdiomPart`
- `mutation_return_mode`'s substring matching — replaced by `ReturnMode`
- `leading_table_references`, `table_references_after_keyword`,
  `relate_edge_table_name` — replaced by lowered statement fields (including
  the thin-statement table refs)
- the four copies of `node_text`/`node_span`/`is_identifier_like`
- `expression.rs`'s `node.kind()` string-matching core — becomes a match on
  `ast::Expr` (the *inference rules* survive; the node-scraping goes)
- most of `AnalysisContext`'s source-text helpers (`text_for_node` et al.) —
  only lowering reads source text; ctx keeps schema/env/row-table/diagnostics

Survives unchanged:

- the entire function signature table (373 files; input is `&[Kind]`, already
  syntax-independent — dispatch swaps `node: Node` for `call: &ast::Call` per D6)
- `ResponseShape`, `PartialReason`, upstream `Kind` usage
- shape-building logic (`response_shape_for_target`, `object_shape_for_*`,
  omit/fetch/split application) — input types change, rules don't
- `StatementEnv`, schema index, the analyzer file layout
- old `semantic.rs` orchestration keeps working during migration exactly as it
  did during the analyzer-tree migration

New capability unlocked (not in scope here, noted for motivation):
`TypeExpr` conversion can finally support `array<string>`, unions, optionals —
today's `UnsupportedSyntax` gap in schema extraction.

## Migration plan

Same strategy that worked for the `semantic.rs` → `analyzer/` migration: the old
path stays green while slices move; every step lands with the full suite passing.

`[v2: coexistence made explicit.]` Steps 2–6 necessarily leave two
implementations alive at once (node-based and AST-based). Rule for the interim:
**the node-based copies are frozen** — any behavior fix during migration lands
in the AST-based implementation only, so drift is one-directional and the old
copy dies without ever having become the more-correct one.

1. **`ast` + `lower` skeleton in `crates/syntax`** — `Spanned`, `PartialNode`;
   lower expressions + idioms + literals only. Unit tests assert lowered shapes,
   including: partial lowering of ERROR-containing input, and the `unhandled`
   bucket collecting an unconsumed clause (the two structural tests of D4).
2. **AST-based expression inference** — implement `infer_expression_fact*` over
   `ast::Expr` *alongside* the node-based `expression.rs` (its callers in
   `semantic.rs` still pass `Node`s at this point — it cannot be replaced yet,
   only paralleled). Equivalence pinned by porting the existing behavior tests.
   The node-based copy is frozen per the rule above and dies in step 7.
3. **Lower SELECT; port `analyzer::data::select`** — absorbs `select_ir.rs`.
   The graph-traversal and brace-selector logic collapses onto `IdiomPart`;
   this step should show the largest net deletion in the diff.
4. **Lower mutations; port the six mutation analyzers** — kills the keyword
   scanners and `.contains` classification for the mutation path.
5. **Flow statements (LET/RETURN/IF/FOR/Block)** — these were still stubs in the
   analyzer tree anyway; they get built directly against the AST (no port).
   Block/IF/FOR analyzers dispatch children per-statement (Goals bullet 5).
6. **Schema statements + `TypeExpr`** — port schema extraction; close the
   `array<string>` gap as its acceptance test.
7. **Rewire orchestration; delete dead code** — `semantic.rs`'s walkers consume
   `Script` instead of re-walking the CST; the frozen node-based expression
   engine and keyword scanners are deleted; `tree-sitter` leaves the workspace
   crate's dependency list (mechanical doneness check).

Each step is independently landable. If we stop after step 4, the codebase is
strictly better than today.

## Open questions (decide before step 1)

`[v2: v1's Q1 (partiality style) is resolved in D4 — variants only, wrapper
deleted. v1's Q3 (statement source order) is trivially satisfied by `Vec` and
removed. Remaining:]`

1. **How much of record-id literals to model** — record ids can be composite
   (`person:['a', 1]`, ranges — grammar: `RecordIdRange`, `RangeRecordId`).
   Propose: table + opaque id span now, structured ids when something consumes
   them.
2. **Grammar gaps discovered during lowering** — lowering will inevitably find
   constructs the grammar parses oddly (it's our grammar — fixes go to
   `tree-sitter-surrealql`). Process question: batch grammar fixes, or fix
   inline as found? Propose inline, since we own the sibling repo.
3. **Closures** (`Closure` node — the grammar models params, pipe, body, and
   return type) — model now or keep `Expr::Closure(PartialNode)`? The 13
   deliberately-`Any` function analyzers (`array::filter`/`map`/`fold`/...)
   need closure typing to improve. Propose: `PartialNode` now; closure typing
   is its own design conversation later.

## Precedent

This is the standard shape for production language tooling: rust-analyzer
(rowan CST → typed `ast` layer → HIR), TypeScript (concrete syntax → bound
tree), roslyn (green/red trees → bound nodes). We are small enough to do the
simplest version: one owned AST, one lowering pass, spans everywhere.

## Completion status (2026-07-04)

| Step | Status |
|---|---|
| 1. `ast` + `lower` skeleton, expressions/idioms/literals | done |
| 2. AST expression inference + D6 function signatures | done |
| 3. SELECT lowering + analyzer, wired live | done |
| 4. Mutations lowering + analyzers, wired live | done |
| 5. Flow statements (LET/RETURN/IF/FOR/Block/...), built AST-native | done |
| 6. Schema statements + `TypeExpr` (closed `array<string>`/`option<T>`/unions/literal types) | done |
| 7. Orchestration + deletions | done for type inference — see below |
| D7 amendment: response types are `Kind` | done |

Type inference has **zero** node-based paths: every statement kind lowers to
`ast::*` and dispatches through `analyzer::statement::analyze_lowered_statement`.
The node dispatcher, its expression shims, `SelectIr`-based shape inference,
all `->`-scanning/substring classification, and `ResponseShape` are deleted.

What intentionally remains node-based (all in `crates/workspace/src/semantic.rs`
plus its helpers in `expression.rs`/`select_ir.rs`):

- the diagnostic validators (table references, projection/function-call/
  assignability/graph checks) and their `.contains`-free gating,
- param inference (the statement-sequence walker collects `$param` uses from
  every nested expression — coverage the type-inference analyzers do not
  replicate),
- schema extraction's statement walking (its *type parsing* is structural via
  `TypeExpr` now).

These are frozen deliberately: the diagnostics phase redistributes each
validator into the analyzer that owns its statement (with finding codes), so
porting them to AST inside `semantic.rs` first would be work done twice.
`tree-sitter` therefore stays a workspace-crate dependency until that phase
completes; removing it remains the doneness check for the *full* old-engine
retirement.

Grammar gaps found during lowering (fix in `tree-sitter-surrealql`):
`tags[*]`, `tags[$]`, closure syntax (`|$x| ...`), `UNSET` clauses, and
`SHOW ... SINCE` all produce ERROR nodes today; each lowers to an explicit
partial.

## Post-completion revision (2026-07-04): the `unhandled` bucket was removed

D4's "Placement 2" (`unhandled: Vec<PartialNode>` on every statement struct)
turned out to be semantically wrong in practice: analyzers poisoned the whole
response type whenever the bucket was non-empty, so a clause that *cannot*
affect a statement's type (`TIMEOUT` on CREATE, `WITH`/`VERSION` on SELECT,
permissions clauses on DEFINE) destroyed an otherwise fully-known type.

The replacement rule is simpler and correct:

- Clauses that cannot affect the response type are consumed by lowering
  without record. Flagging invalid-but-parseable clauses (`RETURN` on
  SELECT) is validation, not typing.
- A statement whose *direct* syntax is broken (`ERROR`/`MISSING` children)
  lowers to `Statement::Partial` wholesale — analyzers never receive a
  half-parsed structure. Deeper breakage stays fine-grained via
  `Expr::Partial`/`IdiomPart::Partial`.

Enum-variant partiality (Placement 1) is unchanged.
