//! Workspace orchestration: gathers sources, runs schema extraction and
//! per-statement analysis, and assembles the public `AnalysisOutput`
//! (response kinds, inferred params, findings) per source.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use surrealdb_types::Kind;
use surrealguard_diagnostics::{Finding, FindingCode, Severity};
use surrealguard_syntax::ast;
use surrealguard_syntax::ast::visit::{self, Visitor};
use surrealguard_syntax::parse::{
    parse_source, ParseError, ParsedSource, SyntaxDiagnostic, SyntaxDiagnosticKind,
};
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use crate::analyzer::pipeline;
use crate::config::WorkspaceConfig;
use crate::schema::SchemaIndex;
use crate::source_registry::SourceRegistry;

/// A set of registered `.surql` sources plus configuration — the unit
/// analysis runs over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    config: WorkspaceConfig,
    registry: SourceRegistry,
    /// Sources whose text a host runs as a **live query** rather than a
    /// one-shot one (`defineLive`). The text is an ordinary SELECT and is
    /// analyzed as one; this is the only record that it will be wrapped in
    /// `LIVE SELECT`, which accepts far less. Empty for every workspace that
    /// has no host files, which is why it is a set and not a field on the
    /// source.
    live_sources: std::collections::BTreeSet<SourceId>,
}

/// Everything analysis produced for one source: its findings, one record
/// per top-level statement, the host-supplied parameters the source
/// reads, and — when exactly one statement responds — the source's
/// overall response kind.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AnalysisOutput {
    /// Every finding raised for the source, syntax and semantic.
    pub diagnostics: Vec<Finding>,
    /// One record per top-level statement, in source order.
    pub statements: Vec<StatementAnalysis>,
    /// The host-supplied parameters the source reads.
    pub inferred_params: Vec<ParamInference>,
    /// Every `LET` binding the source introduces, in source order and at
    /// every nesting depth (top-level, block, DEFINE FUNCTION body, FOR-loop
    /// body). Editor features (inlay hints, hover, go-to-def) read this to
    /// show and locate a binding's inferred type wherever it lives.
    pub let_bindings: Vec<LetBindingAnalysis>,
    /// Every region of the source over which a guard flow-narrowed a binding,
    /// in analysis order. Editor features read this to answer
    /// *per-occurrence* questions ("what is `$x` **here**?") from the cached
    /// analysis, without re-running it.
    #[serde(default)]
    pub narrowings: Vec<NarrowingAnalysis>,
    /// The source's response kind — present only when exactly one
    /// statement responds.
    pub response_kind: Option<Kind>,
}

/// The whole-workspace result: per-source outputs, every finding in one
/// list, and the schema index built from all sources in order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceAnalysis {
    /// Per-source analysis output, keyed by source id.
    pub sources: BTreeMap<SourceId, AnalysisOutput>,
    /// Every finding across all sources, flattened into one list.
    pub diagnostics: Vec<Finding>,
    /// The schema index built from all sources in source order.
    pub schema: SchemaIndex,
}

/// One top-level statement as consumers see it: its span, a stable kind
/// name (`"select"`, `"define_table"`, ...), the response kind when the
/// statement responds, and SELECT's clause modifiers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatementAnalysis {
    /// Where the statement sits in its source.
    pub span: SourceSpan,
    /// Stable statement-kind name (`"select"`, `"define_table"`, ...).
    pub kind: String,
    /// The statement's response kind, when it responds.
    pub response_kind: Option<Kind>,
    /// SELECT clause modifiers; empty for other statement kinds.
    pub select_modifiers: Vec<SelectModifierAnalysis>,
}

/// A `LET $name = <expr>` binding (or a `FOR $name IN ...` loop variable) as
/// consumers see it: the variable name, the span of the `$name` token, and
/// the kind inference gave the bound value. Editor features (inlay hints,
/// hover) read this to show a binding's inferred type where its source has
/// none written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LetBindingAnalysis {
    /// The bound variable name, without the leading `$`.
    pub name: String,
    /// Where the `$name` token sits in its source.
    pub name_span: SourceSpan,
    /// The inferred kind of the bound value; `None` when undeterminable.
    pub kind: Option<Kind>,
}

/// A flow narrowing as consumers see it: the region of the source over which
/// a guard's refinement is in force, the binding (or `param.field.field` path)
/// it refines, and the kind it refines to.
///
/// This is what makes an editor answer *per occurrence* rather than per
/// binding: a `LET`/param records one kind at its binding site, but a guard
/// splits the rest of the scope into a before (declared) and an after
/// (narrowed). The region starts where the refinement takes effect — past a
/// diverging guard statement, or at the start of the branch body the guard
/// admits — and ends with the enclosing statement sequence, so a narrowing
/// never leaks past the scope that established it.
///
/// Pure editor-feature output: nothing in checking reads it (the analyzer
/// applies narrowings through the environment, not through this record).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NarrowingAnalysis {
    /// The refined binding: a bare name (`x` for `$x`), or the exact
    /// `param.field.field` path a field guard refined (`x.parent`).
    pub path: String,
    /// The region of the source the refinement covers.
    pub span: SourceSpan,
    /// The kind the binding (or path) has inside that region.
    pub kind: Kind,
    /// **Why** — the claim that proved the refinement, canonically written
    /// (`$x != NONE`, `$x = 'a' or $x = 'b'`).
    ///
    /// The fourth field, and the last one to become expressible: under the
    /// hand-written recognizers there was nothing to record. `Narrowing::
    /// StripNone` was an enum variant with no subject and the guard expression
    /// was gone by the time a narrowing was recorded, so an editor could say
    /// what a symbol's kind is *here* and never why it differs from the
    /// declaration. `None` on the recognizer path, which still cannot say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

/// A SELECT clause modifier fact: which clause, where, whether it
/// preserves the row shape (WHERE/ORDER/LIMIT do; GROUP/SPLIT don't),
/// and the literal LIMIT bound when known.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectModifierAnalysis {
    /// Which clause (`"where"`, `"group"`, `"limit"`, ...).
    pub kind: String,
    /// Where the clause sits in its source.
    pub span: SourceSpan,
    /// Whether the clause preserves the row shape (WHERE/ORDER/LIMIT do;
    /// GROUP/SPLIT do not).
    pub row_preserving: bool,
    /// The literal `LIMIT` bound, when the clause is a `LIMIT` with a known
    /// constant.
    pub max_len: Option<u64>,
}

/// A host-supplied parameter the source reads: the kind and value domain
/// its uses constrain it to, whether the host must provide it (no
/// `DEFINE PARAM` default), and every use site. This is the contract a
/// host adapter enforces at the call site.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamInference {
    /// The parameter name, without the leading `$`.
    pub name: String,
    /// The kind every use agrees on, unified across constraint sites.
    pub kind: Option<surrealdb_types::Kind>,
    /// Beyond the kind: an enumerable or bounded value domain, when the
    /// uses imply one (`type::field($f)` → the table's field paths;
    /// `LIMIT $n` → non-negative).
    pub domain: Option<ValueDomain>,
    /// Whether the host must supply the parameter — true unless a
    /// `DEFINE PARAM` default covers it.
    pub required: bool,
    /// Every use site of the parameter.
    pub spans: Vec<SourceSpan>,
}

/// The value domain a parameter constraint carries beyond its kind. Host
/// adapters discharge these at their tier: a typed host narrows to a
/// literal union, a dynamic one emits a runtime guard.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ValueDomain {
    /// One of an enumerable set of values.
    OneOf(Vec<surrealdb_types::Value>),
    /// A numeric range (inclusive bounds; `None` = unbounded).
    Range {
        /// Inclusive lower bound; `None` if unbounded below.
        min: Option<i64>,
        /// Inclusive upper bound; `None` if unbounded above.
        max: Option<i64>,
    },
}

impl Workspace {
    /// An empty workspace configured with `config`.
    pub fn new(config: WorkspaceConfig) -> Self {
        Self {
            config,
            registry: SourceRegistry::default(),
            live_sources: std::collections::BTreeSet::new(),
        }
    }

    /// The workspace's resolved configuration.
    pub fn config(&self) -> &WorkspaceConfig {
        &self.config
    }

    /// The source registry backing the workspace.
    pub fn registry(&self) -> &SourceRegistry {
        &self.registry
    }

    /// Mutable access to the source registry, for registering sources
    /// directly.
    pub fn registry_mut(&mut self) -> &mut SourceRegistry {
        &mut self.registry
    }

    /// Registers or updates a file source, returning its [`SourceId`].
    pub fn add_file_source(&mut self, path: std::path::PathBuf, text: String) -> SourceId {
        self.registry.add_file(path, text)
    }

    /// Registers a virtual (non-file) source, returning a fresh
    /// [`SourceId`].
    pub fn add_virtual_source(&mut self, name: String, text: String) -> SourceId {
        self.registry.add_virtual(name, text)
    }

    /// Records that `source`'s text will be run as a live query, so analysis
    /// holds it to [`the live contract`](crate::analyzer::data::live_contract)
    /// on top of everything it checks about a SELECT.
    pub fn mark_live_query(&mut self, source: &SourceId) {
        self.live_sources.insert(source.clone());
    }

    /// Whether `source` was marked by [`Self::mark_live_query`].
    pub fn is_live_query(&self, source: &SourceId) -> bool {
        self.live_sources.contains(source)
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new(WorkspaceConfig::default())
    }
}

/// Registers `query_text` as a virtual source and analyzes it against the
/// workspace's existing schema.
pub fn analyze_query(workspace: &mut Workspace, query_text: &str) -> AnalysisOutput {
    let source_id = workspace.add_virtual_source("query".into(), query_text.into());
    analyze_source(workspace, source_id)
}

/// Parses and analyzes a single registered source, returning its findings,
/// per-statement records, inferred params, and response kind.
pub fn analyze_source(workspace: &Workspace, source: SourceId) -> AnalysisOutput {
    let Some(text) = workspace.registry.text(&source) else {
        return AnalysisOutput::default();
    };

    match parse_source(source.clone(), text) {
        Ok(parsed) => {
            let mut output = AnalysisOutput {
                diagnostics: parsed
                    .syntax_diagnostics()
                    .iter()
                    .map(syntax_diagnostic_to_finding)
                    .collect(),
                ..AnalysisOutput::default()
            };
            let mut pipeline_output = pipeline::analyze_sources_with(
                std::slice::from_ref(&parsed),
                workspace.config().diagnostics.require_suppression_reasons,
                workspace.config().analysis.target_version(),
            );
            output.diagnostics.extend(pipeline_output.diagnostics);
            if let Some(analysis) = pipeline_output.sources.remove(&source) {
                output.response_kind = single_response_kind(&analysis.statements);
                output.statements = analysis.statements;
                output.inferred_params = analysis.params;
                output.let_bindings = analysis.let_bindings;
                output.narrowings = analysis.narrowings;
            }
            output
        }
        Err(error) => AnalysisOutput {
            diagnostics: vec![parse_error_to_finding(source, error)],
            ..AnalysisOutput::default()
        },
    }
}

/// A source's overall response kind is meaningful only when exactly one
/// statement responds.
fn single_response_kind(statements: &[StatementAnalysis]) -> Option<Kind> {
    let mut responding = statements
        .iter()
        .filter_map(|statement| statement.response_kind.clone());
    match (responding.next(), responding.next()) {
        (Some(kind), None) => Some(kind),
        _ => None,
    }
}

/// Analyzes every registered source together, building one shared schema
/// index and returning per-source outputs plus all findings.
pub fn analyze_workspace(workspace: &Workspace) -> WorkspaceAnalysis {
    let mut sources = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let mut parsed_sources = Vec::new();

    for source in workspace.registry.source_ids() {
        let Some(text) = workspace.registry.text(source) else {
            continue;
        };
        match parse_source(source.clone(), text) {
            Ok(parsed) => {
                let output = AnalysisOutput {
                    diagnostics: parsed
                        .syntax_diagnostics()
                        .iter()
                        .map(syntax_diagnostic_to_finding)
                        .collect(),
                    ..AnalysisOutput::default()
                };
                diagnostics.extend(output.diagnostics.iter().cloned());
                sources.insert(source.clone(), output);
                parsed_sources.push(parsed);
            }
            Err(error) => {
                let finding = parse_error_to_finding(source.clone(), error);
                diagnostics.push(finding.clone());
                sources.insert(
                    source.clone(),
                    AnalysisOutput {
                        diagnostics: vec![finding],
                        ..AnalysisOutput::default()
                    },
                );
            }
        }
    }

    // The live contract, for the sources a host runs as a subscription. It is
    // a post-pass rather than part of the pipeline because it is not a
    // property of the SurrealQL: the same SELECT is correct through
    // `defineQuery` and wrong through `defineLive`, and only the host knows
    // which sink it reached.
    for parsed in &parsed_sources {
        if !workspace.is_live_query(parsed.source_id()) {
            continue;
        }
        let mut live = Vec::new();
        for statement in &surrealguard_syntax::lower::lower(parsed).statements {
            if let surrealguard_syntax::ast::Statement::Select(select) = &statement.node {
                crate::analyzer::data::live_contract::check_live_select(
                    select,
                    parsed.source_id(),
                    &mut live,
                );
            }
        }
        if let Some(output) = sources.get_mut(parsed.source_id()) {
            output.diagnostics.extend(live.iter().cloned());
        }
        diagnostics.extend(live);
    }

    let pipeline_output = pipeline::analyze_sources_with(
        &parsed_sources,
        workspace.config().diagnostics.require_suppression_reasons,
        workspace.config().analysis.target_version(),
    );
    for (source, analysis) in pipeline_output.sources {
        if let Some(source_output) = sources.get_mut(&source) {
            source_output.response_kind = single_response_kind(&analysis.statements);
            source_output.statements = analysis.statements;
            source_output.inferred_params = analysis.params;
            source_output.let_bindings = analysis.let_bindings;
            source_output.narrowings = analysis.narrowings;
        }
    }
    for diagnostic in &pipeline_output.diagnostics {
        if let Some(source_output) = sources.get_mut(diagnostic.span().source()) {
            source_output.diagnostics.push(diagnostic.clone());
        }
    }
    diagnostics.extend(pipeline_output.diagnostics);

    WorkspaceAnalysis {
        sources,
        diagnostics,
        schema: pipeline_output.schema,
    }
}

/// The reusable, order-independent cross-source catalog the pre-passes build:
/// everything a single source's analysis reads about the *rest* of the
/// workspace. Build it once with [`build_global_catalog`] and re-analyze one
/// dirty source against it with [`analyze_one_source`], instead of re-running
/// the whole-workspace pass on every keystroke.
pub use crate::analyzer::pipeline::GlobalCatalog;

/// Builds the reusable [`GlobalCatalog`] from a set of parsed sources — the
/// order-independent pre-passes (global `DEFINE` namespace, function returns,
/// untyped-field value kinds, implicit tables, `fn::` guardedness). This is the
/// expensive, source-set-wide part of analysis; cache it and reuse it across
/// edits that don't change any schema-defining source.
pub fn build_global_catalog<P: std::borrow::Borrow<ParsedSource>>(
    parsed_sources: &[P],
) -> GlobalCatalog {
    crate::analyzer::pipeline::build_global_catalog(parsed_sources)
}

/// Re-analyzes ONE source against a prebuilt [`GlobalCatalog`], reproducing the
/// [`AnalysisOutput`] the whole-workspace pass ([`analyze_workspace`]) would
/// produce for that source — without rebuilding the schema or touching any
/// other source.
///
/// Soundness contract: the source must contribute nothing to `global` — no
/// additive `DEFINE`, no implicit-table target (CREATE/UPSERT/INSERT/DELETE or
/// `DEFINE FIELD ... ON`), no `fn::` definition, and no schema effect
/// (`REMOVE`/`ALTER`). A pure query source satisfies this. When it does, the
/// output is byte-identical to `analyze_workspace(...).sources[source]`. Callers
/// must gate on the contract and fall back to the full pass otherwise.
pub fn analyze_one_source(
    global: &GlobalCatalog,
    parsed: &surrealguard_syntax::parse::ParsedSource,
    require_suppression_reasons: bool,
) -> AnalysisOutput {
    let mut output = AnalysisOutput {
        diagnostics: parsed
            .syntax_diagnostics()
            .iter()
            .map(syntax_diagnostic_to_finding)
            .collect(),
        ..AnalysisOutput::default()
    };
    let one = pipeline::analyze_one_source(global, parsed, require_suppression_reasons);
    output.diagnostics.extend(one.diagnostics);
    output.response_kind = single_response_kind(&one.analysis.statements);
    output.statements = one.analysis.statements;
    output.inferred_params = one.analysis.params;
    output.let_bindings = one.analysis.let_bindings;
    output.narrowings = one.analysis.narrowings;
    output
}

// ============================================================================
// Symbol-level incremental re-analysis
// ============================================================================
//
// Editing a *schema* source (any `DEFINE`) used to force a whole-workspace
// re-analysis, because any file could depend on the change. Symbol-level
// invalidation narrows that: it diffs the cheap pre-built [`GlobalCatalog`]
// (`build_global_catalog`, O(defines)) to learn WHICH catalog symbols actually
// changed, then re-analyzes only the sources whose analysis could read one of
// them. The rest keep their previous per-source output verbatim.
//
// Soundness is the whole point — a stale diagnostic is unacceptable — so both
// halves err toward MORE work:
//   * [`changed_symbols`] reports a symbol changed whenever its analysis-
//     relevant shape differs (added, removed, or a callers-visible property).
//   * [`source_reference_set`] is a conservative SUPERSET of the catalog
//     symbols a source could read; when a construct is unanalyzable it degrades
//     to "depends on everything" and is always re-run.
// An over-set only costs extra re-analysis; an under-set is a correctness bug.

/// A catalog symbol whose analysis-relevant shape a source's analysis can read:
/// a table, one of its fields, or a `fn::` function. The unit of change tracking
/// for symbol-level invalidation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SymbolKey {
    /// A table by name (defined or implicit).
    Table(String),
    /// A field by `(table, dotted-path)`.
    Field(String, String),
    /// A `fn::` function by its full path.
    Func(String),
}

/// The catalog symbols whose analysis-relevant shape differs between two
/// catalogs — added, removed, or reshaped in a way a reader's analysis could
/// observe.
///
/// A symbol is "changed" when:
///   * **function** — its parameter kinds, declared `return_kind`, inferred
///     return, or guardedness differ (the properties a call site or the 5009
///     cycle check reads). Callee lists and spans are ignored: a body edit that
///     rewires an internal call without changing the signature does not change
///     what *callers* see (cycle-graph changes are handled separately by
///     [`sources_with_changed_cycle_findings`]).
///   * **field** — its `kind`, `reference`, or `computed` flag differ (what a
///     reader's type inference and reference traversal consume). A field
///     add/remove also flips its table's field-key set, below.
///   * **table** — its field-key SET, relation endpoints, `schemafull`, or
///     `drop` differ, or it was added/removed (defined OR implicit).
///
/// Purely cosmetic differences (spans, comments, defining source) are ignored.
pub fn changed_symbols(old: &GlobalCatalog, new: &GlobalCatalog) -> BTreeSet<SymbolKey> {
    use crate::schema::{FieldDef, FunctionDef, TableDef};

    let mut changed = BTreeSet::new();
    let old_schema = &old.global_defined;
    let new_schema = &new.global_defined;

    // ---- functions ----
    let fn_shape = |f: &FunctionDef| {
        (
            f.args.clone(),
            f.return_kind.clone(),
            f.inferred_return.clone(),
        )
    };
    let fn_names: BTreeSet<&String> = old_schema
        .functions
        .keys()
        .chain(new_schema.functions.keys())
        .collect();
    for name in fn_names {
        let old_fn = old_schema.functions.get(name);
        let new_fn = new_schema.functions.get(name);
        let guard_changed = old.fn_guarded.get(name) != new.fn_guarded.get(name);
        let shape_changed = match (old_fn, new_fn) {
            (Some(a), Some(b)) => fn_shape(a) != fn_shape(b),
            _ => true, // added or removed
        };
        if shape_changed || guard_changed {
            changed.insert(SymbolKey::Func(name.clone()));
        }
    }

    // ---- tables + fields ----
    let table_shape = |t: &TableDef| {
        (
            t.fields.keys().cloned().collect::<BTreeSet<_>>(),
            t.relation
                .as_ref()
                .map(|r| (r.in_tables.clone(), r.out_tables.clone())),
            t.schemafull,
            t.drop_table,
        )
    };
    let field_shape = |f: &FieldDef| (f.kind.clone(), f.reference, f.computed);

    let table_names: BTreeSet<&String> = old_schema
        .tables
        .keys()
        .chain(new_schema.tables.keys())
        .collect();
    for name in table_names {
        match (old_schema.tables.get(name), new_schema.tables.get(name)) {
            (Some(a), Some(b)) => {
                if table_shape(a) != table_shape(b) {
                    changed.insert(SymbolKey::Table(name.clone()));
                }
                // Per-field shape (a field-key add/remove already flipped the
                // table shape above; this catches an in-place TYPE/flag change).
                let field_keys: BTreeSet<&String> =
                    a.fields.keys().chain(b.fields.keys()).collect();
                for key in field_keys {
                    let differ = match (a.fields.get(key), b.fields.get(key)) {
                        (Some(fa), Some(fb)) => field_shape(fa) != field_shape(fb),
                        _ => true,
                    };
                    if differ {
                        changed.insert(SymbolKey::Field(name.clone(), key.clone()));
                    }
                }
            }
            _ => {
                changed.insert(SymbolKey::Table(name.clone()));
            }
        }
    }

    // ---- implicit (schemaless, on-demand) tables ----
    let implicit_set = |catalog: &GlobalCatalog| {
        catalog
            .implicit_tables
            .iter()
            .map(|table| table.name.clone())
            .collect::<BTreeSet<_>>()
    };
    let old_implicit = implicit_set(old);
    let new_implicit = implicit_set(new);
    for name in old_implicit.symmetric_difference(&new_implicit) {
        changed.insert(SymbolKey::Table(name.clone()));
    }

    changed
}

/// A conservative SUPERSET of the catalog symbols a single source's analysis
/// could read, plus two escape hatches for reads it cannot pin down precisely.
#[derive(Clone, Debug, Default)]
pub struct SourceReferenceSet {
    /// Tables and functions the source mentions (as `Table`/`Func`). Fields are
    /// tracked coarsely through their table (see [`Self::is_affected_by`]).
    pub symbols: BTreeSet<SymbolKey>,
    /// The source navigates records (a graph step, a multi-hop field path, a
    /// value-rooted idiom, a `FETCH`, ...), so it could read a field of a table
    /// it never names — any field change is therefore potentially relevant.
    pub navigates_records: bool,
    /// The source contains an unmodeled construct whose reads cannot be
    /// enumerated; it re-runs on any catalog change.
    pub depends_on_all: bool,
}

impl SourceReferenceSet {
    /// Whether any symbol in `changed` is (conservatively) one this source could
    /// read, so the source must be re-analyzed.
    pub fn is_affected_by(&self, changed: &BTreeSet<SymbolKey>) -> bool {
        if changed.is_empty() {
            return false;
        }
        if self.depends_on_all {
            return true;
        }
        for symbol in changed {
            match symbol {
                SymbolKey::Table(_) | SymbolKey::Func(_) => {
                    if self.symbols.contains(symbol) {
                        return true;
                    }
                }
                SymbolKey::Field(table, _) => {
                    // A field is read either through its own table (named here)
                    // or through a record link from some other table.
                    if self.navigates_records
                        || self.symbols.contains(&SymbolKey::Table(table.clone()))
                    {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Builds the [`SourceReferenceSet`] for one source: a conservative superset of
/// every catalog symbol its analysis could read. Walks the lowered AST and
/// collects every table name (FROM targets, `record<T>`, `DEFINE ... ON`, graph
/// edges, RELATE endpoints, DEFINE TABLE, ...) and every `fn::` called or
/// defined. Field reads are tracked coarsely: a bare single-hop field read is
/// attributed to its named table, while any record-navigating construct sets
/// [`SourceReferenceSet::navigates_records`] so a field change anywhere is
/// treated as relevant. Unmodeled constructs set
/// [`SourceReferenceSet::depends_on_all`].
pub fn source_reference_set(parsed: &ParsedSource) -> SourceReferenceSet {
    let mut refs = SourceReferenceSet::default();
    for stmt in &surrealguard_syntax::lower::lower_statements(parsed) {
        refs.visit_statement(stmt);
    }
    refs
}

impl SourceReferenceSet {
    fn add_table(&mut self, name: &str) {
        self.symbols.insert(SymbolKey::Table(name.to_string()));
    }
}

/// The reference-set collector is a [`Visitor`]: the default traversal reaches
/// every expression, and the overrides below record what each construct could
/// read. Anything unmodeled ([`Visitor::visit_partial`]) means the reads
/// cannot be enumerated at all.
impl Visitor for SourceReferenceSet {
    fn visit_table_ref(&mut self, name: &ast::Spanned<String>) {
        self.add_table(&name.node);
    }

    fn visit_partial(&mut self, _partial: &ast::PartialNode) {
        self.depends_on_all = true;
    }

    fn visit_statement(&mut self, statement: &ast::Spanned<ast::Statement>) {
        // Order-sensitive schema effects force the full path upstream
        // (`source_requires_full_reanalysis`); nothing precise to collect.
        if matches!(
            statement.node,
            ast::Statement::Remove(_) | ast::Statement::Alter(_)
        ) {
            self.depends_on_all = true;
        }
        visit::walk_statement(self, statement);
    }

    fn visit_select(&mut self, select: &ast::SelectStmt) {
        // `FETCH` expands record links, reading fields of tables the statement
        // never names.
        if !select.fetch.is_empty() {
            self.navigates_records = true;
        }
        visit::walk_select(self, select);
    }

    fn visit_live_select(&mut self, live: &ast::LiveSelectStmt) {
        if !live.fetch.is_empty() {
            self.navigates_records = true;
        }
        visit::walk_live_select(self, live);
    }

    fn visit_define_table(&mut self, table: &ast::DefineTable) {
        // The declaration itself: a change to this table is a change to the
        // source that defines it.
        self.add_table(&table.name.node);
        visit::walk_define_table(self, table);
    }

    fn visit_define_function(&mut self, function: &ast::DefineFunction) {
        self.symbols
            .insert(SymbolKey::Func(function.name.node.clone()));
        visit::walk_define_function(self, function);
    }

    fn visit_call(&mut self, call: &ast::Call) {
        if call.path.node.starts_with("fn::") {
            self.symbols.insert(SymbolKey::Func(call.path.node.clone()));
        }
        visit::walk_call(self, call);
    }

    fn visit_idiom(&mut self, idiom: &ast::Idiom) {
        // A field read that hops more than once, traverses a graph/reference
        // edge, or is rooted in a value could reach a field of a table this
        // source never names — mark it as record-navigating so any field
        // change is relevant.
        let mut field_hops = 0usize;
        for part in &idiom.parts {
            match &part.node {
                ast::IdiomPart::Field(_) => field_hops += 1,
                ast::IdiomPart::Start(_)
                | ast::IdiomPart::Graph { .. }
                | ast::IdiomPart::Destructure(_)
                | ast::IdiomPart::Where(_)
                | ast::IdiomPart::Method { .. }
                | ast::IdiomPart::Recurse { .. } => self.navigates_records = true,
                ast::IdiomPart::Index(_)
                | ast::IdiomPart::All
                | ast::IdiomPart::Last
                | ast::IdiomPart::Optional
                | ast::IdiomPart::Flatten
                | ast::IdiomPart::Partial(_) => {}
            }
        }
        if field_hops >= 2 {
            self.navigates_records = true;
        }
        visit::walk_idiom(self, idiom);
    }

    fn visit_type_expr(&mut self, ty: &ast::Spanned<ast::TypeExpr>) {
        // `record<T>` / `references<T>` name tables directly.
        if let ast::TypeExpr::Parameterized { name, args } = &ty.node {
            if matches!(name.node.as_str(), "record" | "references") {
                self.navigates_records = true;
                for arg in args {
                    if let ast::TypeExpr::Name(table) = &arg.node {
                        self.add_table(&table.node);
                    } else {
                        self.visit_type_expr(arg);
                    }
                }
                return;
            }
        }
        visit::walk_type_expr(self, ty);
    }
}

/// Whether a source contains a schema construct the symbol diff cannot model
/// soundly, so an edit touching it must fall back to a full re-analysis:
///   * `REMOVE` / `ALTER` — order-sensitive, and additive-only catalog diffing
///     cannot see a cross-source removal.
///   * `DEFINE PARAM` — the catalog now carries each param's default fact, but
///     [`SymbolKey`] has no param variant, so a value change is still invisible
///     to [`changed_symbols`] and every reader must be re-walked.
///   * `DEFINE ANALYZER` — analyzer pipelines feed `SEARCH` indexes and are not
///     tracked as reference-set symbols.
///
/// The LSP gates dirty schema documents on this before taking the symbol path.
pub fn source_requires_full_reanalysis(parsed: &ParsedSource) -> bool {
    let statements = surrealguard_syntax::lower::lower_statements(parsed);
    statements.iter().any(|stmt| {
        matches!(
            &stmt.node,
            ast::Statement::Remove(_)
                | ast::Statement::Alter(_)
                | ast::Statement::Define(ast::DefineStmt::Param(_) | ast::DefineStmt::Analyzer(_))
        )
    })
}

/// The sources whose cross-source function-cycle findings (5009) differ between
/// two catalogs. The cycle check spans each finding at the offending function's
/// own definition, so a body edit that creates or breaks a cycle can change the
/// findings of a source that reads none of the changed symbols directly (e.g.
/// the *other* function in a newly-formed mutual-recursion pair). Any such
/// source must join the affected set; comparing the grouped finding sets catches
/// it. Findings in edited (dirty) sources shift spans and are reported too, but
/// those sources are already affected, so the extra entries are harmless.
pub fn sources_with_changed_cycle_findings(
    old: &GlobalCatalog,
    new: &GlobalCatalog,
) -> BTreeSet<SourceId> {
    fn by_source(findings: Vec<Finding>) -> BTreeMap<SourceId, BTreeSet<(u16, String)>> {
        let mut grouped: BTreeMap<SourceId, BTreeSet<(u16, String)>> = BTreeMap::new();
        for finding in findings {
            grouped
                .entry(finding.span().source().clone())
                .or_default()
                .insert((finding.code().number(), finding.message().to_string()));
        }
        grouped
    }

    let old_by = by_source(crate::analyzer::pipeline::function_cycle_findings(old));
    let new_by = by_source(crate::analyzer::pipeline::function_cycle_findings(new));

    let mut changed = BTreeSet::new();
    let sources: BTreeSet<&SourceId> = old_by.keys().chain(new_by.keys()).collect();
    for source in sources {
        if old_by.get(source) != new_by.get(source) {
            changed.insert(source.clone());
        }
    }
    changed
}

/// Re-analyzes ONLY the `affected` sources against a prebuilt [`GlobalCatalog`],
/// reproducing the exact [`AnalysisOutput`] the whole-workspace pass
/// ([`analyze_workspace`]) would produce for each — without re-walking the
/// unaffected sources. Returns one output per affected source.
///
/// Unlike [`analyze_one_source`], this reproduces the whole-workspace loop's
/// per-source `working` base for every affected source (schema or query) and
/// injects each source's own 5009 cycle findings, so a *schema* source that
/// contributes definitions is reproduced correctly. The caller
/// ([`crate::analysis`] consumers / the LSP) must supply an `affected` set that
/// is a superset of every source whose output could differ — see
/// [`changed_symbols`], [`source_reference_set`], and
/// [`sources_with_changed_cycle_findings`].
pub fn reanalyze_sources<P: std::borrow::Borrow<ParsedSource>>(
    parsed_sources: &[P],
    global: &GlobalCatalog,
    affected: &BTreeSet<SourceId>,
    require_suppression_reasons: bool,
) -> BTreeMap<SourceId, AnalysisOutput> {
    let mut per_source = pipeline::reanalyze_sources(
        parsed_sources,
        global,
        affected,
        require_suppression_reasons,
    );

    let mut outputs = BTreeMap::new();
    for parsed in parsed_sources {
        let parsed = parsed.borrow();
        let Some(one) = per_source.remove(parsed.source_id()) else {
            continue;
        };
        let mut output = AnalysisOutput {
            diagnostics: parsed
                .syntax_diagnostics()
                .iter()
                .map(syntax_diagnostic_to_finding)
                .collect(),
            ..AnalysisOutput::default()
        };
        output.diagnostics.extend(one.diagnostics);
        output.response_kind = single_response_kind(&one.analysis.statements);
        output.statements = one.analysis.statements;
        output.inferred_params = one.analysis.params;
        output.let_bindings = one.analysis.let_bindings;
        output.narrowings = one.analysis.narrowings;
        outputs.insert(parsed.source_id().clone(), output);
    }
    outputs
}

/// Rebuilds the workspace [`SchemaIndex`] the whole-workspace pass
/// ([`analyze_workspace`]) would produce, without the per-statement analysis
/// walk. The symbol-incremental path uses this to refresh the cached schema
/// after a schema edit while re-analyzing only the affected sources.
pub fn build_workspace_schema<P: std::borrow::Borrow<ParsedSource>>(
    parsed_sources: &[P],
) -> crate::schema::SchemaIndex {
    pipeline::build_run_schema(parsed_sources)
}

fn syntax_diagnostic_to_finding(diagnostic: &SyntaxDiagnostic) -> Finding {
    let code = match diagnostic.kind() {
        SyntaxDiagnosticKind::ErrorNode => FindingCode::syntax(1),
        SyntaxDiagnosticKind::MissingNode => FindingCode::syntax(2),
    };

    Finding::new(
        diagnostic.span().clone(),
        code,
        Severity::Error,
        diagnostic.message(),
    )
}

fn parse_error_to_finding(source: SourceId, error: ParseError) -> Finding {
    let span = SourceSpan::new(
        source,
        ByteRange::new(0, 0).expect("zero-width byte range is valid"),
    );

    Finding::new(
        span,
        FindingCode::syntax(0),
        Severity::Error,
        error.to_string(),
    )
}
