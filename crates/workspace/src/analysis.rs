//! Workspace orchestration: gathers sources, runs schema extraction and
//! per-statement analysis, and assembles the public `AnalysisOutput`
//! (response kinds, inferred params, findings) per source.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use surrealdb_types::Kind;
use surrealguard_diagnostics::{Finding, FindingCode, Severity};
use surrealguard_syntax::ast;
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
            );
            output.diagnostics.extend(pipeline_output.diagnostics);
            if let Some(analysis) = pipeline_output.sources.remove(&source) {
                output.response_kind = single_response_kind(&analysis.statements);
                output.statements = analysis.statements;
                output.inferred_params = analysis.params;
                output.let_bindings = analysis.let_bindings;
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

    let pipeline_output = pipeline::analyze_sources_with(
        &parsed_sources,
        workspace.config().diagnostics.require_suppression_reasons,
    );
    for (source, analysis) in pipeline_output.sources {
        if let Some(source_output) = sources.get_mut(&source) {
            source_output.response_kind = single_response_kind(&analysis.statements);
            source_output.statements = analysis.statements;
            source_output.inferred_params = analysis.params;
            source_output.let_bindings = analysis.let_bindings;
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
    let statements = surrealguard_syntax::lower::lower_statements(parsed);
    let mut refs = SourceReferenceSet::default();
    for stmt in &statements {
        walk_statement(&stmt.node, &mut refs);
    }
    refs
}

fn add_table(refs: &mut SourceReferenceSet, name: &str) {
    refs.symbols.insert(SymbolKey::Table(name.to_string()));
}

fn walk_statement(stmt: &ast::Statement, refs: &mut SourceReferenceSet) {
    use ast::Statement as S;
    match stmt {
        S::Select(s) => {
            for from in &s.from {
                walk_expr(&from.node, refs);
            }
            for projection in &s.projections {
                walk_projection(projection, refs);
            }
            for idiom in s.omit.iter().chain(&s.split) {
                walk_idiom(&idiom.node, refs);
            }
            if !s.fetch.is_empty() {
                refs.navigates_records = true;
                for idiom in &s.fetch {
                    walk_idiom(&idiom.node, refs);
                }
            }
            walk_opt_expr(&s.where_clause, refs);
            if let Some(order) = &s.order {
                for key in &order.keys {
                    walk_expr(&key.expr.node, refs);
                }
            }
            if let Some(group) = &s.group {
                for key in &group.keys {
                    walk_idiom(&key.node, refs);
                }
            }
            walk_opt_expr(&s.limit, refs);
            walk_opt_expr(&s.start, refs);
            walk_opt_expr(&s.timeout, refs);
        }
        S::Create(s) => {
            walk_targets(&s.targets, refs);
            walk_data(&s.data, refs);
        }
        S::Update(s) => {
            walk_targets(&s.targets, refs);
            walk_data(&s.data, refs);
            walk_opt_expr(&s.where_clause, refs);
        }
        S::Upsert(s) => {
            walk_targets(&s.targets, refs);
            walk_data(&s.data, refs);
            walk_opt_expr(&s.where_clause, refs);
        }
        S::Delete(s) => {
            walk_targets(&s.targets, refs);
            walk_opt_expr(&s.where_clause, refs);
        }
        S::Insert(s) => {
            if let Some(target) = &s.target {
                walk_expr(&target.node, refs);
            }
            match &s.data {
                ast::InsertData::Values(values) => {
                    for value in values {
                        walk_expr(&value.node, refs);
                    }
                }
                ast::InsertData::Rows { rows, .. } => {
                    for row in rows {
                        for (_, value) in row {
                            walk_expr(&value.node, refs);
                        }
                    }
                }
                ast::InsertData::Assignments(assignments) => {
                    for (_, value) in assignments {
                        walk_expr(&value.node, refs);
                    }
                }
                ast::InsertData::Partial(_) => refs.depends_on_all = true,
            }
        }
        S::Relate(s) => {
            for endpoint in [&s.from, &s.edge, &s.to].into_iter().flatten() {
                walk_expr(&endpoint.node, refs);
            }
            walk_data(&s.data, refs);
        }
        S::Define(def) => walk_define(def, refs),
        S::Remove(_) | S::Alter(_) => {
            // Order-sensitive schema effects force the full path upstream
            // (`source_requires_full_reanalysis`); nothing precise to collect.
            refs.depends_on_all = true;
        }
        S::Let(s) => walk_expr(&s.value.node, refs),
        S::Return(s) => walk_opt_expr(&s.value, refs),
        S::IfElse(s) => {
            for branch in &s.branches {
                walk_expr(&branch.condition.node, refs);
                walk_block(&branch.body, refs);
            }
            if let Some(block) = &s.else_branch {
                walk_block(block, refs);
            }
        }
        S::For(s) => {
            walk_expr(&s.iterable.node, refs);
            walk_block(&s.body, refs);
        }
        S::Block(block) => walk_block(block, refs),
        S::Throw(s) => walk_opt_expr(&s.value, refs),
        S::Sleep(s) => walk_opt_expr(&s.duration, refs),
        S::Kill(s) => walk_opt_expr(&s.id, refs),
        S::LiveSelect(s) => {
            if let Some(table) = &s.table {
                add_table(refs, &table.node);
            }
        }
        S::Show(s) => {
            if let Some(table) = &s.table {
                add_table(refs, &table.node);
            }
            walk_opt_expr(&s.since, refs);
        }
        S::Info(s) => {
            if let Some(table) = &s.table {
                add_table(refs, &table.node);
            }
        }
        S::Rebuild(s) => {
            if let Some(table) = &s.table {
                add_table(refs, &table.node);
            }
        }
        S::Expr(expr) => walk_expr(&expr.node, refs),
        // No catalog reads: transaction control, USE, OPTION, BREAK/CONTINUE.
        S::Begin(_) | S::Commit(_) | S::Cancel(_) | S::Use(_) | S::Option(_) | S::Break(_)
        | S::Continue(_) => {}
        S::Partial(_) => refs.depends_on_all = true,
    }
}

fn walk_define(def: &ast::DefineStmt, refs: &mut SourceReferenceSet) {
    use ast::DefineStmt as D;
    match def {
        D::Table(t) => {
            add_table(refs, &t.name.node);
            if let Some(relation) = &t.relation {
                for endpoint in relation.in_tables.iter().chain(&relation.out_tables) {
                    add_table(refs, &endpoint.node);
                }
            }
            for predicate in &t.permissions {
                walk_expr(&predicate.node, refs);
            }
        }
        D::Field(f) => {
            add_table(refs, &f.table.node);
            if let Some(ty) = &f.ty {
                walk_type(&ty.node, refs);
            }
            walk_opt_expr(&f.default, refs);
            walk_opt_expr(&f.value, refs);
            walk_opt_expr(&f.computed, refs);
            walk_opt_expr(&f.assert, refs);
            for predicate in &f.permissions {
                walk_expr(&predicate.node, refs);
            }
        }
        D::Index(i) => {
            add_table(refs, &i.table.node);
            for field in &i.fields {
                walk_idiom(&field.node, refs);
            }
        }
        D::Event(e) => {
            add_table(refs, &e.table.node);
            walk_opt_expr(&e.when, refs);
            walk_opt_expr(&e.then, refs);
        }
        D::Function(func) => {
            refs.symbols.insert(SymbolKey::Func(func.name.node.clone()));
            if let Some(ty) = &func.return_ty {
                walk_type(&ty.node, refs);
            }
            for (_, ty) in &func.params {
                if let Some(ty) = ty {
                    walk_type(&ty.node, refs);
                }
            }
            if let Some(body) = &func.body {
                walk_block(body, refs);
            }
        }
        D::Param(p) => walk_opt_expr(&p.value, refs),
        D::Analyzer(_) => {}
        // Unmodeled DEFINE kinds (ACCESS/API/...): could read anything.
        D::Other(_) => refs.depends_on_all = true,
    }
}

fn walk_targets(targets: &[ast::Spanned<ast::Expr>], refs: &mut SourceReferenceSet) {
    for target in targets {
        walk_expr(&target.node, refs);
    }
}

fn walk_data(data: &Option<ast::DataClause>, refs: &mut SourceReferenceSet) {
    let Some(data) = data else { return };
    match data {
        ast::DataClause::Set(assignments) => {
            for assignment in assignments {
                walk_idiom(&assignment.target.node, refs);
                walk_expr(&assignment.value.node, refs);
            }
        }
        ast::DataClause::Unset(idioms) => {
            for idiom in idioms {
                walk_idiom(&idiom.node, refs);
            }
        }
        ast::DataClause::Content(expr)
        | ast::DataClause::Merge(expr)
        | ast::DataClause::Patch(expr)
        | ast::DataClause::Replace(expr)
        | ast::DataClause::Single(expr) => walk_expr(&expr.node, refs),
        ast::DataClause::Partial(_) => refs.depends_on_all = true,
    }
}

fn walk_projection(projection: &ast::Projection, refs: &mut SourceReferenceSet) {
    match projection {
        ast::Projection::Wildcard(_) => {}
        ast::Projection::Expr { expr, .. } => walk_expr(&expr.node, refs),
        ast::Projection::Partial(_) => refs.depends_on_all = true,
    }
}

fn walk_block(block: &ast::Block, refs: &mut SourceReferenceSet) {
    for stmt in &block.statements {
        walk_statement(&stmt.node, refs);
    }
}

fn walk_opt_expr(expr: &Option<ast::Spanned<ast::Expr>>, refs: &mut SourceReferenceSet) {
    if let Some(expr) = expr {
        walk_expr(&expr.node, refs);
    }
}

fn walk_expr(expr: &ast::Expr, refs: &mut SourceReferenceSet) {
    use ast::Expr as E;
    match expr {
        E::Literal(_) | E::Param(_) => {}
        E::Table(name) => add_table(refs, &name.node),
        E::RecordId { table, .. } => add_table(refs, &table.node),
        E::Idiom(idiom) => walk_idiom(idiom, refs),
        E::Binary { lhs, rhs, .. } => {
            walk_expr(&lhs.node, refs);
            walk_expr(&rhs.node, refs);
        }
        E::Prefix { expr, .. } => walk_expr(&expr.node, refs),
        E::Call(call) => {
            if call.path.node.starts_with("fn::") {
                refs.symbols
                    .insert(SymbolKey::Func(call.path.node.clone()));
            }
            for arg in &call.args {
                walk_expr(&arg.node, refs);
            }
        }
        E::Object(fields) => {
            for (_, value) in fields {
                walk_expr(&value.node, refs);
            }
        }
        E::Array(items) => {
            for item in items {
                walk_expr(&item.node, refs);
            }
        }
        E::Subquery(stmt) => walk_statement(&stmt.node, refs),
        E::Block(block) => walk_block(block, refs),
        E::Cast { ty, expr } => {
            walk_type(&ty.node, refs);
            walk_expr(&expr.node, refs);
        }
        E::Closure(closure) => {
            if let Some(ty) = &closure.return_ty {
                walk_type(&ty.node, refs);
            }
            for (_, ty) in &closure.params {
                if let Some(ty) = ty {
                    walk_type(&ty.node, refs);
                }
            }
            walk_expr(&closure.body.node, refs);
        }
        E::Partial(_) => refs.depends_on_all = true,
    }
}

fn walk_idiom(idiom: &ast::Idiom, refs: &mut SourceReferenceSet) {
    // A field read that hops more than once, traverses a graph/reference edge,
    // or is rooted in a value could reach a field of a table this source never
    // names — mark it as record-navigating so any field change is relevant.
    let mut field_hops = 0usize;
    for part in &idiom.parts {
        match &part.node {
            ast::IdiomPart::Start(expr) => {
                refs.navigates_records = true;
                walk_expr(&expr.node, refs);
            }
            ast::IdiomPart::Field(_) => field_hops += 1,
            ast::IdiomPart::Index(expr) => walk_expr(&expr.node, refs),
            ast::IdiomPart::Graph { step, .. } => {
                refs.navigates_records = true;
                for target in &step.targets {
                    add_table(refs, &target.node);
                }
                if let Some(where_clause) = &step.where_clause {
                    walk_expr(&where_clause.node, refs);
                }
            }
            ast::IdiomPart::Destructure(idioms) => {
                refs.navigates_records = true;
                for inner in idioms {
                    walk_idiom(&inner.node, refs);
                }
            }
            ast::IdiomPart::Where(expr) => {
                refs.navigates_records = true;
                walk_expr(&expr.node, refs);
            }
            ast::IdiomPart::Method { args, .. } => {
                refs.navigates_records = true;
                for arg in args {
                    walk_expr(&arg.node, refs);
                }
            }
            ast::IdiomPart::Recurse { .. } => refs.navigates_records = true,
            ast::IdiomPart::All | ast::IdiomPart::Last | ast::IdiomPart::Optional => {}
            ast::IdiomPart::Partial(_) => refs.depends_on_all = true,
        }
    }
    if field_hops >= 2 {
        refs.navigates_records = true;
    }
}

fn walk_type(ty: &ast::TypeExpr, refs: &mut SourceReferenceSet) {
    use ast::TypeExpr as T;
    match ty {
        T::Name(_) => {}
        T::Parameterized { name, args } => {
            // `record<T>` / `references<T>` name tables directly.
            if matches!(name.node.as_str(), "record" | "references") {
                refs.navigates_records = true;
                for arg in args {
                    if let T::Name(table) = &arg.node {
                        add_table(refs, &table.node);
                    } else {
                        walk_type(&arg.node, refs);
                    }
                }
            } else {
                for arg in args {
                    walk_type(&arg.node, refs);
                }
            }
        }
        T::Union(variants) => {
            for variant in variants {
                walk_type(&variant.node, refs);
            }
        }
        T::Optional(inner) => walk_type(&inner.node, refs),
        T::Object(fields) => {
            for (_, field_ty) in fields {
                walk_type(&field_ty.node, refs);
            }
        }
        T::Literal(_) => {}
        T::Partial(_) => refs.depends_on_all = true,
    }
}

/// Whether a source contains a schema construct the symbol diff cannot model
/// soundly, so an edit touching it must fall back to a full re-analysis:
///   * `REMOVE` / `ALTER` — order-sensitive, and additive-only catalog diffing
///     cannot see a cross-source removal.
///   * `DEFINE PARAM` — a parameter's *value* (hence a reader's inferred kind)
///     is not captured by [`GlobalCatalog`], so a value change is invisible to
///     [`changed_symbols`].
///   * `DEFINE ANALYZER` — analyzer pipelines feed `SEARCH` indexes and are not
///     tracked as reference-set symbols.
/// The LSP gates dirty schema documents on this before taking the symbol path.
pub fn source_requires_full_reanalysis(parsed: &ParsedSource) -> bool {
    let statements = surrealguard_syntax::lower::lower_statements(parsed);
    statements.iter().any(|stmt| {
        matches!(
            &stmt.node,
            ast::Statement::Remove(_)
                | ast::Statement::Alter(_)
                | ast::Statement::Define(ast::DefineStmt::Param(_))
                | ast::Statement::Define(ast::DefineStmt::Analyzer(_))
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
    let mut per_source =
        pipeline::reanalyze_sources(parsed_sources, global, affected, require_suppression_reasons);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expression::PartialReason;
    use surrealdb_types::Kind;
    use surrealdb_types::KindLiteral;

    #[test]
    fn analyze_query_returns_statement_analysis_for_parseable_surrealql() {
        let mut workspace = Workspace::default();

        let output = analyze_query(&mut workspace, "SELECT * FROM person;");

        // One-shot queries get the full pipeline: with no schema in the
        // workspace, the unknown table is a real finding, alongside the two
        // opt-in whole-table/`SELECT *` lints (7014/7015, allow-by-default).
        assert_eq!(output.diagnostics.len(), 3);
        assert!(output
            .diagnostics
            .iter()
            .any(|finding| finding.code().to_string() == "E1001"));
        assert!(output
            .diagnostics
            .iter()
            .any(|finding| finding.code().number() == 7014));
        assert!(output
            .diagnostics
            .iter()
            .any(|finding| finding.code().number() == 7015));
        assert_eq!(output.statements.len(), 1);
        assert_eq!(output.statements[0].kind, "select");
        assert_eq!(
            output.statements[0].span.source().as_str(),
            "virtual://query#0"
        );
        assert_eq!(output.statements[0].span.range().start(), 0);
        assert_eq!(output.statements[0].span.range().end(), 20);
        assert_eq!(output.statements[0].response_kind, Some(Kind::Any));
        assert!(output.inferred_params.is_empty());
        assert_eq!(output.response_kind, Some(Kind::Any));
    }

    #[test]
    fn analyze_query_emits_statement_analysis_for_all_parseable_statement_kinds() {
        let mut workspace = Workspace::default();
        let query = r#"
BEGIN;
CANCEL;
COMMIT;
INFO FOR DB;
KILL 'abc';
LIVE SELECT * FROM person;
SHOW CHANGES FOR TABLE person;
SLEEP 1s;
USE NS app DB app;
OPTION IMPORT;
BREAK;
CONTINUE;
FOR $item IN [1] { RETURN $item; };
THROW 'bad';
IF true { RETURN 1; };
LET $name = 'Ada';
DELETE person;
CREATE person;
SELECT * FROM person;
RELATE person:one->likes->post:one;
UPDATE person SET name = 'Ada';
REMOVE TABLE person;
UPSERT person:one SET name = 'Ada';
RETURN 1;
ALTER TABLE person SCHEMAFULL;
DEFINE TABLE person;
REBUILD INDEX by_name ON TABLE person;
INSERT INTO person { name: 'Ada' };
"#;

        let output = analyze_query(&mut workspace, query);

        // The fixture deliberately trips contracts (bare BREAK, KILL with a
        // string, tables used before definition); this test pins only the
        // statement-kind vocabulary.
        let kinds: Vec<_> = output
            .statements
            .iter()
            .map(|statement| statement.kind.as_str())
            .collect();
        assert_eq!(
            kinds,
            vec![
                "begin",
                "cancel",
                "commit",
                "info_for",
                "kill",
                "live_select",
                "show",
                "sleep",
                "use",
                "option",
                "break",
                "continue",
                "for",
                "throw",
                "if_else",
                "let",
                "delete",
                "create",
                "select",
                "relate",
                "update",
                "remove",
                "upsert",
                "return",
                "alter",
                "define_table",
                "rebuild",
                "insert",
            ]
        );
    }

    #[test]
    fn analyze_query_surfaces_syntax_diagnostics_with_source_spans() {
        let mut workspace = Workspace::default();

        let output = analyze_query(&mut workspace, "SELECT * FROM ;");

        assert_eq!(output.diagnostics.len(), 1);
        let diagnostic = &output.diagnostics[0];
        assert_eq!(diagnostic.code(), FindingCode::syntax(1));
        assert_eq!(diagnostic.severity(), Severity::Error);
        assert_eq!(diagnostic.span().source().as_str(), "virtual://query#0");
        assert!(!diagnostic.message().is_empty());
    }

    #[test]
    fn analyze_source_uses_existing_registered_source_id() {
        let mut workspace = Workspace::default();
        let source_id =
            workspace.add_virtual_source("schema".into(), "DEFINE TABLE person;".into());

        let output = analyze_source(&workspace, source_id.clone());

        assert_eq!(
            workspace.registry().text(&source_id),
            Some("DEFINE TABLE person;")
        );
        assert!(output.diagnostics.is_empty());
    }

    #[test]
    fn select_from_a_record_param_types_against_its_table() {
        // `SELECT … FROM ONLY $p` where `$p: record<T>` projects against T's
        // fields, not `Any`.
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE org SCHEMAFULL;\n\
             DEFINE TABLE unit SCHEMAFULL;\n\
             DEFINE FIELD org ON unit TYPE record<org>;\n\
             DEFINE FUNCTION fn::u($id: record<unit>) { RETURN (SELECT org FROM ONLY $id); };"
                .into(),
        );
        let query = workspace.add_virtual_source("query".into(), "RETURN fn::u($x);".into());

        let output = analyze_workspace(&workspace);
        let rendered = crate::render_kind(
            output.sources[&query]
                .response_kind
                .as_ref()
                .expect("response kind"),
        );
        assert!(
            rendered.contains("org: record<org>"),
            "expected the projected field typed, got {rendered}"
        );
    }

    #[test]
    fn computed_back_reference_index_selects_the_element_record() {
        // `COMPUTED <~T[0]` selects ONE back-reference record, not the whole
        // `array<record<T>>` the bare `<~T` yields.
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE organization_billing SCHEMAFULL;\n\
             DEFINE FIELD org ON organization_billing TYPE record<organization> REFERENCE;\n\
             DEFINE TABLE organization SCHEMAFULL;\n\
             DEFINE FIELD billing ON organization COMPUTED <~organization_billing[0];"
                .into(),
        );
        let query =
            workspace.add_virtual_source("query".into(), "SELECT billing FROM organization;".into());

        let output = analyze_workspace(&workspace);
        let rendered = crate::render_kind(
            output.sources[&query]
                .response_kind
                .as_ref()
                .expect("response kind"),
        );
        assert!(
            rendered.contains("billing: record<organization_billing>"),
            "expected the element record, got {rendered}"
        );
    }

    #[test]
    fn if_expression_in_value_position_types_as_the_branch_union() {
        // An IF used as a value (`RETURN IF …`, which lowers to a subquery)
        // types as the union of its branch values — not `unknown`.
        let mut workspace = Workspace::default();
        let query =
            workspace.add_virtual_source("query".into(), "RETURN IF $c { 1 } ELSE { 'x' };".into());

        let output = analyze_workspace(&workspace);

        assert_eq!(
            output.sources[&query].response_kind,
            Some(Kind::Either(vec![Kind::Int, Kind::String])),
        );
    }

    #[test]
    fn cross_source_udf_call_resolves_a_table_bearing_body_return() {
        // A function whose body reads a table, defined in one source and called
        // from another: the call must resolve the real return type, not `Any`.
        // (The cross-source catalog import used to infer the body against an
        // empty schema, degrading table-bearing bodies to `Any`.)
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE unit SCHEMAFULL;\n\
             DEFINE FIELD label ON unit TYPE string;\n\
             DEFINE FUNCTION fn::pick() { RETURN (SELECT VALUE label FROM ONLY unit); };"
                .into(),
        );
        let query = workspace.add_virtual_source("query".into(), "RETURN fn::pick();".into());

        let output = analyze_workspace(&workspace);

        assert_eq!(
            output.sources[&query].response_kind,
            Some(Kind::String),
            "cross-source UDF call must resolve its table-bearing body return"
        );
    }

    #[test]
    fn analyze_workspace_returns_outputs_for_all_registered_sources() {
        let mut workspace = Workspace::default();
        let good = workspace.add_virtual_source("schema".into(), "DEFINE TABLE person;".into());
        let bad = workspace.add_virtual_source("query".into(), "SELECT * FROM ;".into());

        let output = analyze_workspace(&workspace);

        assert_eq!(output.sources.len(), 2);
        assert!(output.sources[&good].diagnostics.is_empty());
        assert_eq!(output.sources[&bad].diagnostics.len(), 1);
        assert_eq!(output.diagnostics.len(), 1);
    }

    #[test]
    fn analyze_workspace_indexes_define_table_declarations() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE TABLE company SCHEMAFULL;".into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(output.schema.tables.len(), 2);
        assert_eq!(output.schema.tables["person"].name, "person");
        assert_eq!(output.schema.tables["company"].source, source);
        let span = &output.schema.tables["person"].name_span;
        assert_eq!(span.source(), &source);
        assert_eq!(span.range().start(), 13);
        assert_eq!(span.range().end(), 19);
    }

    #[test]
    fn analyze_workspace_reports_duplicate_table_declarations() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE TABLE person;".into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(output.schema.tables.len(), 1);
        let duplicates: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1022))
            .collect();
        assert_eq!(duplicates.len(), 1);
        assert_eq!(
            duplicates[0].message(),
            "`person` is already defined; this DEFINE silently replaces the earlier one"
        );
        assert_eq!(duplicates[0].span().range().start(), 34);
        assert_eq!(duplicates[0].span().range().end(), 40);
    }

    fn codes(output: &WorkspaceAnalysis, number: u16) -> usize {
        output
            .diagnostics
            .iter()
            .filter(|finding| finding.code().number() == number)
            .count()
    }

    // ---- 4013: GROUP BY field not in projections ----

    #[test]
    fn group_by_key_not_projected_fires_4013_once() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE sale;\nDEFINE FIELD region ON sale TYPE string;\nDEFINE FIELD amount ON sale TYPE int;".into(),
        );
        // `region` is grouped but never projected, so the grouped rows carry no
        // region label — a footgun SurrealDB runs silently.
        workspace.add_virtual_source(
            "query".into(),
            "SELECT math::sum(amount) AS total FROM sale GROUP BY region;".into(),
        );
        let output = analyze_workspace(&workspace);
        assert_eq!(codes(&output, 4013), 1, "{:?}", output.diagnostics);
    }

    #[test]
    fn group_by_key_projected_stays_silent_for_4013() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE sale;\nDEFINE FIELD region ON sale TYPE string;\nDEFINE FIELD amount ON sale TYPE int;".into(),
        );
        // `region` is projected (bare) and `year` is projected as an alias that
        // the GROUP BY names — both forms cover the grouping key.
        workspace.add_virtual_source(
            "query".into(),
            "SELECT region, math::sum(amount) AS total FROM sale GROUP BY region;".into(),
        );
        let output = analyze_workspace(&workspace);
        assert_eq!(codes(&output, 4013), 0, "{:?}", output.diagnostics);
    }

    // ---- 6002: LET shadows a DEFINE PARAM with a different kind ----

    #[test]
    fn let_shadows_define_param_with_incompatible_kind_fires_6002_once() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE PARAM $min_age VALUE 18;\nLET $min_age = 'old';\nRETURN $min_age;".into(),
        );
        let output = analyze_workspace(&workspace);
        assert_eq!(codes(&output, 6002), 1, "{:?}", output.diagnostics);
    }

    #[test]
    fn let_shadows_define_param_with_compatible_kind_stays_silent_for_6002() {
        let mut workspace = Workspace::default();
        // Same int kind — a benign rebind, no clash.
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE PARAM $min_age VALUE 18;\nLET $min_age = 21;\nRETURN $min_age;".into(),
        );
        let output = analyze_workspace(&workspace);
        assert_eq!(codes(&output, 6002), 0, "{:?}", output.diagnostics);
    }

    // ---- 7001: unused LET binding (opt-in; default allow) ----

    #[test]
    fn unused_let_fires_7001_once() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "LET $unused = 1;\nRETURN 5;".into(),
        );
        let output = analyze_workspace(&workspace);
        assert_eq!(codes(&output, 7001), 1, "{:?}", output.diagnostics);
    }

    #[test]
    fn used_let_stays_silent_for_7001() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "LET $used = 1;\nRETURN $used;".into(),
        );
        let output = analyze_workspace(&workspace);
        assert_eq!(codes(&output, 7001), 0, "{:?}", output.diagnostics);
    }

    #[test]
    fn let_used_only_in_nested_block_stays_silent_for_7001() {
        let mut workspace = Workspace::default();
        // The reference lives in a nested block that follows the LET — the
        // textual scan of the later sibling must find it.
        workspace.add_virtual_source(
            "query".into(),
            "LET $used = 1;\nIF true { RETURN $used; };".into(),
        );
        let output = analyze_workspace(&workspace);
        assert_eq!(codes(&output, 7001), 0, "{:?}", output.diagnostics);
    }

    #[test]
    fn field_element_type_definition_is_not_a_duplicate_of_the_base_field() {
        let mut workspace = Workspace::default();
        // `arr[*]` types the ELEMENTS of the `arr` array — distinct from the
        // base field, though both collapse to the same dotted path. It must
        // not be flagged as a duplicate definition (1022). A genuine plain
        // redefinition still is.
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD arr ON t TYPE array;\n\
             DEFINE FIELD arr[*] ON t TYPE object;\n\
             DEFINE FIELD arr[*].price ON t TYPE string;\n\
             DEFINE FIELD dup ON t TYPE int;\n\
             DEFINE FIELD dup ON t TYPE int;"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        let duplicates: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1022))
            .collect();
        // Only the genuine plain `dup` redefinition is a duplicate.
        assert_eq!(duplicates.len(), 1, "unexpected duplicates: {duplicates:?}");
        assert!(duplicates[0].message().contains("dup"));
    }

    #[test]
    fn a_valid_write_to_a_field_with_descendant_definitions_is_not_a_type_error() {
        let mut workspace = Workspace::default();
        // Two of the most common real schema shapes. A descendant definition
        // used to REPLACE the parent's declared kind, so `items` read as a bare
        // object and `cfg` lost its optionality — both of these writes were
        // error-severity 2001s on valid SurrealQL, which aborts `generate` for
        // the whole project.
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD items ON t TYPE array<object>;\n\
             DEFINE FIELD items[*] ON t TYPE object;\n\
             DEFINE FIELD items[*].price ON t TYPE string;\n\
             DEFINE FIELD cfg ON t TYPE option<object>;\n\
             DEFINE FIELD cfg.theme ON t TYPE string;\n\
             CREATE t SET items = [{ price: '1' }], cfg = NONE;\n\
             CREATE t SET items = [], cfg = { theme: 'dark' };"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(codes(&output, 2001), 0, "{:?}", output.diagnostics);
    }

    #[test]
    fn a_wrong_write_to_a_refined_field_is_still_a_type_error() {
        let mut workspace = Workspace::default();
        // Refining must not turn into blanket permissiveness: the element type
        // the descendants declare is enforced, and the parent's own shape is
        // still a contract.
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD items ON t TYPE array<object>;\n\
             DEFINE FIELD items[*].price ON t TYPE string;\n\
             CREATE t SET items = [{ price: 1 }];\n\
             CREATE t SET items = 'nope';"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(codes(&output, 2001), 2, "{:?}", output.diagnostics);
    }

    #[test]
    fn a_subfield_under_a_scalar_parent_fires_1025_and_leaves_the_parent_writable() {
        let mut workspace = Workspace::default();
        // `title` is a string, so `title.sub` describes a member that can never
        // exist. Report the definition (1025) — and keep `title` a string, so
        // the perfectly valid `SET title = 'hello'` stays valid.
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD title ON t TYPE string;\n\
             DEFINE FIELD title.sub ON t TYPE string;\n\
             CREATE t SET title = 'hello';"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(codes(&output, 1025), 1, "{:?}", output.diagnostics);
        assert_eq!(codes(&output, 2001), 0, "{:?}", output.diagnostics);
    }

    #[test]
    fn a_subfield_under_an_object_shaped_or_undeclared_parent_never_fires_1025() {
        let mut workspace = Workspace::default();
        // Every shape with room for a subfield stays silent: an undeclared
        // parent, a bare/FLEXIBLE `object`, an `option<object>`, a literal
        // object, an untyped parent, and a collection reached through `[*]`.
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD undeclared.name ON t TYPE string;\n\
             DEFINE FIELD open ON t TYPE object;\n\
             DEFINE FIELD open.name ON t TYPE string;\n\
             DEFINE FIELD flex ON t FLEXIBLE TYPE object;\n\
             DEFINE FIELD flex.name ON t TYPE string;\n\
             DEFINE FIELD maybe ON t TYPE option<object>;\n\
             DEFINE FIELD maybe.name ON t TYPE string;\n\
             DEFINE FIELD shaped ON t TYPE { name: string };\n\
             DEFINE FIELD shaped.age ON t TYPE int;\n\
             DEFINE FIELD untyped ON t VALUE {};\n\
             DEFINE FIELD untyped.name ON t TYPE string;\n\
             DEFINE FIELD items ON t TYPE array;\n\
             DEFINE FIELD items[*].sku ON t TYPE string;"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(codes(&output, 1025), 0, "{:?}", output.diagnostics);
    }

    #[test]
    fn record_field_referencing_undefined_table_fires_1001() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD owner ON t TYPE record<ghost>;"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(codes(&output, 1001), 1);
        let finding = output
            .diagnostics
            .iter()
            .find(|f| f.code().number() == 1001)
            .expect("a 1001 finding");
        assert!(finding.message().contains("ghost"));
    }

    #[test]
    fn record_field_referencing_defined_tables_is_clean() {
        let mut workspace = Workspace::default();
        // Defined targets, including through option/array/union wrappers, must
        // never fire 1001.
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE TABLE org SCHEMAFULL;\n\
             DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD owner ON t TYPE record<user>;\n\
             DEFINE FIELD maybe ON t TYPE option<record<user>>;\n\
             DEFINE FIELD many ON t TYPE array<record<user | org>>;"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(codes(&output, 1001), 0, "unexpected: {:?}", output.diagnostics);
    }

    #[test]
    fn default_violating_own_assert_fires_2037() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD status ON t TYPE string \
                 DEFAULT 'activ' ASSERT $value IN ['active', 'inactive'];"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(codes(&output, 2037), 1, "unexpected: {:?}", output.diagnostics);
    }

    #[test]
    fn default_satisfying_own_assert_is_clean() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD status ON t TYPE string \
                 DEFAULT 'active' ASSERT $value IN ['active', 'inactive'];\n\
             DEFINE FIELD score ON t TYPE int DEFAULT 5 ASSERT $value >= 0 AND $value <= 10;"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(codes(&output, 2037), 0, "unexpected: {:?}", output.diagnostics);
    }

    #[test]
    fn non_const_default_or_assert_does_not_fire_2037() {
        let mut workspace = Workspace::default();
        // A non-const DEFAULT (function call) and a non-foldable ASSERT term
        // must both BAIL — never guess.
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD created ON t TYPE datetime \
                 DEFAULT time::now() ASSERT $value < time::now();\n\
             DEFINE FIELD name ON t TYPE string \
                 DEFAULT 'x' ASSERT string::len($value) > 0;"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(codes(&output, 2037), 0, "unexpected: {:?}", output.diagnostics);
    }

    #[test]
    fn bare_count_resolves_without_unknown_function() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person SCHEMAFULL;".into(),
        );

        // Bare `count()` must resolve to the builtin (no spurious 5001), and a
        // grouped count is a real aggregate (no 4023 either).
        let output = analyze_query(
            &mut workspace,
            "SELECT count() AS n FROM person GROUP ALL;",
        );

        assert_eq!(
            output
                .diagnostics
                .iter()
                .filter(|f| f.code().number() == 5001)
                .count(),
            0,
            "unexpected 5001: {:?}",
            output.diagnostics
        );
        assert_eq!(
            output
                .diagnostics
                .iter()
                .filter(|f| f.code().number() == 4023)
                .count(),
            0,
        );
    }

    #[test]
    fn ungrouped_bare_count_fires_4023_not_5001() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person SCHEMAFULL;".into(),
        );

        let output = analyze_query(&mut workspace, "SELECT count() AS n FROM person;");

        assert_eq!(
            output
                .diagnostics
                .iter()
                .filter(|f| f.code().number() == 5001)
                .count(),
            0,
            "unexpected 5001: {:?}",
            output.diagnostics
        );
        assert_eq!(
            output
                .diagnostics
                .iter()
                .filter(|f| f.code().number() == 4023)
                .count(),
            1,
            "expected 4023: {:?}",
            output.diagnostics
        );
    }

    #[test]
    fn unique_and_plain_index_over_the_same_fields_are_not_redundant() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD code ON t TYPE string;\n\
             DEFINE INDEX by_code ON t FIELDS code;\n\
             DEFINE INDEX unique_code ON t FIELDS code UNIQUE;"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        // A UNIQUE index and a plain index over the same field do different
        // work, so 1029 must not fire.
        assert_eq!(codes(&output, 1029), 0);
    }

    #[test]
    fn two_plain_indexes_over_the_same_fields_are_redundant() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD code ON t TYPE string;\n\
             DEFINE INDEX by_code ON t FIELDS code;\n\
             DEFINE INDEX also_code ON t FIELDS code;"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(codes(&output, 1029), 1);
    }

    #[test]
    fn event_then_block_ending_in_let_is_statement_position_not_a_value_block() {
        let mut workspace = Workspace::default();
        // An event THEN body is in statement position; a block ending in LET
        // is fine there, so the value-block lint (4017) must not fire.
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD name ON t TYPE string;\n\
             DEFINE EVENT ev ON t WHEN $event = 'CREATE' THEN {\n\
                 LET $x = CREATE t SET name = 'a';\n\
             };"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(codes(&output, 4017), 0);
    }

    #[test]
    fn none_guard_narrows_the_right_operand_for_arg_checks() {
        let mut workspace = Workspace::default();
        // `$value = NONE OR string::len($value) = 10`: inside the right
        // operand `$value` is non-none, so `string::len` sees `string`, not
        // `option<string>` — no argument-type finding (5002).
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD code ON t TYPE option<string>\n\
                 ASSERT $value = NONE OR string::len($value) = 10;\n\
             DEFINE FIELD mail ON t TYPE option<string>\n\
                 ASSERT $value != NONE AND string::is_email($value);"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(codes(&output, 5002), 0, "{:?}", output.diagnostics);
    }

    #[test]
    fn an_unguarded_optional_argument_still_fails_the_arg_check() {
        let mut workspace = Workspace::default();
        // Without the `= NONE OR` guard the optional reaches `string::len`
        // as `option<string>`, which is a genuine 5002.
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD code ON t TYPE option<string>\n\
                 ASSERT string::len($value) = 10;"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        assert_eq!(codes(&output, 5002), 1);
    }

    #[test]
    fn analyze_workspace_indexes_define_table_relation_metadata() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;".into(),
        );

        let output = analyze_workspace(&workspace);
        let relation = output.schema.tables["likes"]
            .relation
            .as_ref()
            .expect("likes is indexed as relation table");

        assert_eq!(relation.in_tables, vec!["person"]);
        assert_eq!(relation.out_tables, vec!["post"]);
    }

    #[test]
    fn analyze_workspace_indexes_schemafull_field_declarations() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD profile.name ON person TYPE string;"
                .into(),
        );

        let output = analyze_workspace(&workspace);

        let fields = &output.schema.tables["person"].fields;
        assert_eq!(fields.len(), 1);
        let field = &fields["profile.name"];
        assert_eq!(field.path, vec!["profile", "name"]);
        assert_eq!(field.table, "person");
        assert_eq!(field.kind, Some(Kind::String));
        assert_eq!(field.source, source);
        assert_eq!(field.name_span.range().start(), 45);
        assert_eq!(field.name_span.range().end(), 57);
        assert_eq!(field.table_span.range().start(), 61);
        assert_eq!(field.table_span.range().end(), 67);
    }

    #[test]
    fn analyze_workspace_validates_define_index_table_and_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD profile.email ON person TYPE string;\nDEFINE INDEX by_name ON TABLE person FIELDS name, profile.email;".into(),
        );

        let output = analyze_workspace(&workspace);

        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1002)));
        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1002)));
    }

    #[test]
    fn analyze_workspace_reports_define_index_unknown_table_and_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE INDEX by_email ON TABLE person FIELDS email;\nDEFINE INDEX missing_table_idx ON TABLE ghost FIELDS name;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code().number(), 1001 | 1002))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "`person` has no field `email` (used by index `by_email`)",
                "index `missing_table_idx` is defined on `ghost`, which is not a defined table",
            ]
        );
    }

    #[test]
    fn analyze_workspace_validates_define_event_table_and_when_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE EVENT adult ON TABLE person WHEN $event.age >= 18 THEN { UPDATE person SET age = $event.age; };".into(),
        );

        let output = analyze_workspace(&workspace);

        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1002)));
        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1002)));
    }

    #[test]
    fn analyze_workspace_reports_define_event_unknown_table_and_when_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE EVENT bad_field ON TABLE person WHEN $event.missing >= 18 THEN { RETURN true; };\nDEFINE EVENT bad_table ON TABLE ghost WHEN $event.age > 18 THEN { RETURN true; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code().number(), 1001 | 1002))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "event `bad_field` references unknown field `missing` on table `person`",
                "event `bad_table` targets unknown table `ghost`",
            ]
        );
    }

    #[test]
    fn analyze_workspace_validates_rebuild_and_remove_index_targets() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE INDEX by_name ON TABLE person FIELDS name;\nREBUILD INDEX by_name ON TABLE person;\nREMOVE INDEX by_name ON TABLE person;\nREBUILD INDEX missing ON TABLE person;\nREMOVE INDEX by_name ON TABLE ghost;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code().number() == 1012)
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "`person` has no index `missing` (REBUILD)",
                "REMOVE targets `ghost`, which is not a defined table",
            ]
        );
    }

    #[test]
    fn analyze_workspace_validates_remove_table_and_field_targets() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD profile.email ON person TYPE string;\nREMOVE FIELD name ON TABLE person;\nREMOVE FIELD profile.email ON person;\nREMOVE TABLE ghost;\nREMOVE FIELD missing ON TABLE person;\nREMOVE FIELD name ON TABLE ghost;\nREMOVE TABLE person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code(), code if code == FindingCode::schema(1021) || code == FindingCode::schema(1021)))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "REMOVE TABLE `ghost` targets a table that doesn't exist",
                "`person` has no field `missing` to remove",
                "REMOVE FIELD `name` targets `ghost`, which is not a defined table",
            ]
        );
    }

    #[test]
    fn analyze_workspace_validates_alter_table_targets() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nALTER TABLE person DROP;\nALTER TABLE ghost DROP;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(messages, vec!["`ghost` is not a defined table"]);
    }

    #[test]
    fn analyze_workspace_reports_fields_on_unknown_tables() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE FIELD name ON person TYPE string;".into(),
        );

        let output = analyze_workspace(&workspace);

        // A `DEFINE FIELD ... ON` a never-`DEFINE`d table is valid: the table
        // exists schemaless. No unknown-table diagnostic fires, and no
        // `DEFINE TABLE` was seen so the catalog stays empty.
        assert!(output.schema.tables.is_empty());
        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1001)));
    }

    #[test]
    fn analyze_workspace_resolves_structured_field_types() {
        // Parameterized/union/option types resolve to real kinds instead
        // of degrading to UnsupportedSyntax.
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD tags ON person TYPE array<string>;\nDEFINE FIELD age ON person TYPE option<int>;\nDEFINE FIELD status ON person TYPE 'active' | 'inactive';".into(),
        );

        let output = analyze_workspace(&workspace);

        let fields = &output.schema.tables["person"].fields;
        assert_eq!(
            fields["tags"].kind,
            Some(Kind::Array(Box::new(Kind::String), None))
        );
        assert_eq!(
            fields["age"].kind,
            Some(Kind::Either(vec![Kind::None, Kind::Int]))
        );
        assert_eq!(
            fields["status"].kind,
            Some(Kind::Either(vec![
                Kind::Literal(KindLiteral::String("active".into())),
                Kind::Literal(KindLiteral::String("inactive".into())),
            ]))
        );
        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::param(6003)));
    }

    #[test]
    fn analyze_workspace_marks_unsupported_field_type_syntax_as_partial_analysis() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person;\nDEFINE FIELD shape ON person TYPE geometry<point>;".into(),
        );

        let output = analyze_workspace(&workspace);

        let field = &output.schema.tables["person"].fields["shape"];
        assert!(field.kind.is_none());
        assert_eq!(
            field.partial,
            vec![PartialReason::UnsupportedSyntax("geometry<...>".into())]
        );
        let partial: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::param(6003))
            .collect();
        assert_eq!(partial.len(), 1);
    }

    #[test]
    fn analyze_workspace_validates_select_table_references_against_schema() {
        let mut workspace = Workspace::default();
        let query = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nSELECT * FROM company;".into(),
        );

        let output = analyze_workspace(&workspace);

        let unknown_tables: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .collect();
        assert_eq!(unknown_tables.len(), 1);
        assert_eq!(unknown_tables[0].message(), "`company` is not a defined table");
        assert_eq!(unknown_tables[0].span().source(), &query);
        assert_eq!(unknown_tables[0].span().range().start(), 35);
        assert_eq!(unknown_tables[0].span().range().end(), 42);
        // The unknown table (1001) plus the two opt-in `SELECT *`/whole-table
        // lints (7014/7015, allow-by-default but emitted as raw findings).
        assert_eq!(output.sources[&query].diagnostics.len(), 3);
    }

    #[test]
    fn analyze_workspace_allows_known_basic_table_references() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nSELECT * FROM person;\nCREATE person;\nUPDATE person SET name = 'A';\nDELETE person;".into(),
        );

        let output = analyze_workspace(&workspace);

        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1002)));
    }

    #[test]
    fn analyze_workspace_treats_removed_table_as_absent_for_later_references() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nREMOVE TABLE person;\nUPDATE person SET name = 'Ada';".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(messages, vec!["`person` is not a defined table"]);
        assert!(output.schema.table("person").is_none());
    }

    #[test]
    fn analyze_workspace_does_not_use_later_table_definitions_for_earlier_statements() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "UPDATE person SET name = 'Ada';\nDEFINE TABLE person;\nUPDATE person SET name = 'Grace';".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_tables: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(unknown_tables, vec!["`person` is not a defined table"]);
        assert!(output.schema.table("person").is_some());
    }

    #[test]
    fn analyze_workspace_applies_field_definitions_and_removals_in_source_order() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person SCHEMAFULL;\nUPDATE person SET name = 'Ada';\nDEFINE FIELD name ON person TYPE int;\nUPDATE person SET name = 'Grace';\nREMOVE FIELD name ON person;\nUPDATE person SET name = 'Hedy';".into(),
        );

        let output = analyze_workspace(&workspace);
        let type_mismatches: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::type_error(2001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            type_mismatches,
            vec!["`name` is declared `int`, but this value is `string`".to_string()]
        );
        assert!(output
            .schema
            .field("person", &crate::schema::FieldPath::parse("name"))
            .is_none());
    }

    #[test]
    fn analyze_workspace_overwrite_replaces_definition_for_downstream_statements() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE user;\nDEFINE TABLE org;\nDEFINE TABLE person TYPE RELATION IN user OUT org;\nRELATE org:acme->person->user:drew;\nDEFINE TABLE OVERWRITE person TYPE RELATION IN org OUT user;\nRELATE org:acme->person->user:drew;".into(),
        );

        let output = analyze_workspace(&workspace);
        let endpoint_messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::graph(3002))
            .map(|finding| finding.message().to_string())
            .collect();

        // The first RELATE (before OVERWRITE) contradicts the declared
        // shape; the second (after) matches the overwritten relation.
        assert_eq!(
            endpoint_messages,
            vec![
                "relation `person` connects `user`->`person`->`org`, but this RELATE writes `org`->`person`->`user`",
            ]
        );
        let relation = output
            .schema
            .table("person")
            .and_then(|table| table.relation.as_ref())
            .unwrap();
        assert_eq!(relation.in_tables, vec!["org".to_string()]);
        assert_eq!(relation.out_tables, vec!["user".to_string()]);
    }

    #[test]
    fn analyze_workspace_validates_create_update_and_delete_table_references() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "CREATE ghost;\nUPDATE phantom SET seen = true;\nDELETE missing;".into(),
        );

        let output = analyze_workspace(&workspace);

        // CREATE and DELETE create/act on schemaless tables on demand, so a
        // never-`DEFINE`d target is valid there. UPDATE only ever touches
        // existing rows, so its unknown target still reports.
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .map(|finding| finding.message().to_string())
            .collect();
        assert_eq!(messages, vec!["`phantom` is not a defined table"]);
    }

    #[test]
    fn analyze_workspace_validates_table_references_for_non_select_statement_forms() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "UPSERT ghost SET seen = true;\nINSERT INTO phantom { seen: true };\nLIVE SELECT * FROM missing;\nALTER TABLE shadow SCHEMAFULL;\nREMOVE TABLE stale;\nREBUILD INDEX by_name ON TABLE absent;\nSHOW CHANGES FOR TABLE vanished;\nINFO FOR TABLE hidden;\nINFO FOR TB obscured;".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_tables: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code() == FindingCode::schema(1001))
            .map(|diagnostic| diagnostic.message().to_string())
            .collect();

        // UPSERT and INSERT write schemaless tables on demand, so `ghost` and
        // `phantom` are valid targets. The read/DDL forms below (LIVE SELECT,
        // ALTER, REBUILD, SHOW, INFO) still require the table to exist.
        assert_eq!(
            unknown_tables,
            vec![
                "`missing` is not a defined table",
                "`shadow` is not a defined table",
                "`absent` is not a defined table",
                "`vanished` is not a defined table",
                "`hidden` is not a defined table",
                "`obscured` is not a defined table",
            ]
        );
    }

    #[test]
    fn analyze_workspace_emits_statement_analysis_for_registered_sources() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nSELECT * FROM person;\nCREATE person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];

        let kinds: Vec<_> = source_output
            .statements
            .iter()
            .map(|statement| statement.kind.as_str())
            .collect();
        assert_eq!(kinds, vec!["define_table", "select", "create"]);
        assert_eq!(source_output.statements[1].span.source(), &source);
        assert_eq!(source_output.statements[1].span.range().start(), 21);
        assert_eq!(source_output.statements[1].span.range().end(), 41);
        assert!(matches!(
            source_output.statements[1].response_kind,
            Some(Kind::Any)
        ));
    }

    #[test]
    fn analyze_workspace_collects_query_parameters_by_name_with_spans() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nSELECT * FROM person WHERE id = $id OR owner = $id AND team = $team;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;

        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "id");
        assert_eq!(params[0].kind, None);
        assert!(params[0].required);
        assert_eq!(params[0].spans.len(), 2);
        assert_eq!(params[0].spans[0].range().start(), 53);
        assert_eq!(params[0].spans[0].range().end(), 56);
        assert_eq!(params[0].spans[1].range().start(), 68);
        assert_eq!(params[0].spans[1].range().end(), 71);
        assert_eq!(params[1].name, "team");
        assert_eq!(params[1].spans.len(), 1);
        assert_eq!(params[1].spans[0].range().start(), 83);
        assert_eq!(params[1].spans[0].range().end(), 88);
    }

    #[test]
    fn analyze_workspace_excludes_let_variables_from_query_params() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $age = 42;\nRETURN $age;\nSELECT * FROM person WHERE name = $name;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;

        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "name");
    }

    #[test]
    fn analyze_workspace_seeds_auth_as_a_bound_fact_not_a_host_param() {
        // `$auth` is engine-supplied (the authenticated record, or NONE), never
        // host-supplied. A query comparing against it must not list it among
        // the inferred params — only the genuine host param `$name` remains.
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE post;\nSELECT * FROM post WHERE owner = $auth AND title = $name;"
                .into(),
        );

        let output = analyze_workspace(&workspace);
        let names: Vec<_> = output.sources[&source]
            .inferred_params
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, vec!["name"], "`$auth` is engine-supplied");
    }

    #[test]
    fn analyze_workspace_seeds_all_session_params_not_as_host_params() {
        // `$session`/`$access`/`$token`/`$scope` are engine-supplied too, so a
        // query that only references those has no host params.
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE post;\nSELECT * FROM post WHERE a = $session AND b = $access AND c = $token AND d = $scope;"
                .into(),
        );

        let output = analyze_workspace(&workspace);
        assert!(
            output.sources[&source].inferred_params.is_empty(),
            "session params are engine-supplied: {:?}",
            output.sources[&source].inferred_params
        );
    }

    #[test]
    fn analyze_workspace_default_auth_on_record_field_stays_clean() {
        // The workshop pattern (`organization.surql`): `DEFAULT $auth` on a
        // `record<account>` field. A field's DEFAULT/VALUE is a write-time
        // context whose session is not statically known, so the top-level
        // `$auth: option<record>` seed must not leak in and there make the
        // DEFAULT a false 2001 (option<record> vs record<account>).
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            concat!(
                "DEFINE TABLE account SCHEMAFULL;\n",
                "DEFINE TABLE organization SCHEMAFULL;\n",
                "DEFINE FIELD owner ON organization TYPE record<account> DEFAULT $auth;\n",
            )
            .into(),
        );
        let output = analyze_workspace(&workspace);
        assert_eq!(codes(&output, 2001), 0, "{:?}", output.diagnostics);
    }

    #[test]
    fn analyze_workspace_value_and_default_auth_field_clauses_stay_clean() {
        // Other workshop field-clause shapes: `VALUE $auth` on a `record<T>`
        // field, and `DEFAULT ALWAYS $auth` on an `option<record<...>>` field.
        // None may raise a type mismatch from the session seed.
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            concat!(
                "DEFINE TABLE account SCHEMAFULL;\n",
                "DEFINE TABLE team SCHEMAFULL;\n",
                "DEFINE TABLE invite SCHEMAFULL;\n",
                "DEFINE FIELD created_by ON invite TYPE record<account> VALUE $auth READONLY;\n",
                "DEFINE FIELD owner ON invite TYPE option<record<account | team>> DEFAULT ALWAYS $auth;\n",
            )
            .into(),
        );
        let output = analyze_workspace(&workspace);
        assert_eq!(codes(&output, 2001), 0, "{:?}", output.diagnostics);
        assert_eq!(codes(&output, 2005), 0, "{:?}", output.diagnostics);
    }

    #[test]
    fn analyze_workspace_auth_member_access_does_not_fire_1002() {
        // `$auth` seeds as an open `option<record>`; member access degrades to
        // partial/Any (`field_of_kind` returns None on an empty-Record target),
        // so `$auth.id` / `$auth.whatever` never raise a false 1002.
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE post;\nSELECT * FROM post WHERE owner = $auth.id AND x = $auth.whatever;"
                .into(),
        );

        let output = analyze_workspace(&workspace);
        assert_eq!(codes(&output, 1002), 0, "{:?}", output.diagnostics);
    }

    #[test]
    fn analyze_workspace_infers_return_shape_from_let_variable() {
        let mut workspace = Workspace::default();
        let source =
            workspace.add_virtual_source("query".into(), "LET $age = 42;\nRETURN $age;".into());

        let output = analyze_workspace(&workspace);
        let return_statement = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "return")
            .expect("return statement exists");

        assert_eq!(return_statement.response_kind, Some(Kind::Int));
    }

    #[test]
    fn analyze_workspace_uses_let_variable_kind_for_mutation_assignability() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $age = 42;\nCREATE person SET age = $age;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];

        assert!(source_output.inferred_params.is_empty());
        assert!(source_output
            .diagnostics
            .iter()
            .all(|finding| finding.code().to_string() != "E2001"));
    }

    #[test]
    fn analyze_workspace_reports_let_variable_kind_mismatch_for_mutation_assignability() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $age = 'old';\nCREATE person SET age = $age;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E2001" && message == "`age` is declared `int`, but this value is `string`"
        }));
    }

    #[test]
    fn analyze_workspace_reports_branch_local_let_mismatch_for_mutation_assignability() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nIF true { LET $age = 'old'; CREATE person SET age = $age; } ELSE { RETURN 0; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E2001" && message == "`age` is declared `int`, but this value is `string`"
        }));
    }

    #[test]
    fn function_arg_mismatch_renders_optional_kinds_idiomatically() {
        let mut workspace = Workspace::default();
        // Passing an `int` where a param declared `option<string>` is expected
        // trips the 5002 arg-type contract. The declared kind must render in
        // the idiomatic compact form (`option<string>`), never the raw union
        // (`none | string`).
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE FUNCTION fn::greet($name: option<string>) { RETURN $name; };\n\
             RETURN fn::greet(1);"
                .into(),
        );

        let output = analyze_workspace(&workspace);
        let finding = output.sources[&source]
            .diagnostics
            .iter()
            .find(|finding| finding.code().number() == 5002)
            .expect("a 5002 argument-type finding");
        let message = finding.message().to_string();

        assert!(
            message.contains("declared `option<string>`"),
            "expected compact optional rendering, got: {message}"
        );
        assert!(
            message.contains("is a `int`"),
            "expected the passed kind, got: {message}"
        );
        assert!(!message.contains("none |"), "raw union leaked: {message}");
        // The arg-mismatch note points back at the DEFINE FUNCTION.
        assert!(
            finding
                .related()
                .iter()
                .any(|note| note.message == "`fn::greet` is defined here"),
            "expected a related note at the function definition, got: {:?}",
            finding.related()
        );
    }

    #[test]
    fn analyze_workspace_infers_let_variables_from_prior_let_variables() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $age = 42;\nLET $next = $age + 1;\nRETURN $next;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];
        let return_statement = source_output
            .statements
            .iter()
            .find(|statement| statement.kind == "return")
            .expect("return statement exists");

        assert!(source_output.inferred_params.is_empty());
        assert_eq!(return_statement.response_kind, Some(Kind::Int));
    }

    #[test]
    fn analyze_workspace_uses_dependent_let_variable_kind_for_mutation_assignability() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $age = 42;\nLET $next = $age + 1;\nCREATE person SET age = $next;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];

        assert!(source_output.inferred_params.is_empty());
        assert!(source_output
            .diagnostics
            .iter()
            .all(|finding| finding.code().to_string() != "E2001"));
    }

    #[test]
    fn analyze_workspace_reports_dependent_let_variable_kind_mismatch() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $name = 'Drew';\nLET $excited = $name + '!';\nCREATE person SET age = $excited;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E2001" && message == "`age` is declared `int`, but this value is `string`"
        }));
    }

    #[test]
    fn analyze_workspace_uses_latest_let_shadow_for_return_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $x = 1;\nLET $x = 's';\nRETURN $x;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];
        let return_statement = source_output
            .statements
            .iter()
            .find(|statement| statement.kind == "return")
            .expect("return statement exists");

        assert!(source_output.inferred_params.is_empty());
        assert_eq!(return_statement.response_kind, Some(Kind::String));
    }

    #[test]
    fn analyze_workspace_preserves_prior_let_dependency_before_shadowing() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $x = 1;\nLET $y = $x + 1;\nLET $x = 's';\nRETURN $y;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];
        let return_statement = source_output
            .statements
            .iter()
            .find(|statement| statement.kind == "return")
            .expect("return statement exists");

        assert!(source_output.inferred_params.is_empty());
        assert_eq!(return_statement.response_kind, Some(Kind::Int));
    }

    #[test]
    fn analyze_workspace_treats_forward_let_use_as_external_param() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $y = $x + 1;\nLET $x = 1;\nRETURN $y;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;

        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "x");
    }

    #[test]
    fn analyze_workspace_infers_if_else_return_shape_union() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            // A dynamic guard keeps both branches reachable (a constant guard
            // would fold to a single branch); this exercises the shape union.
            "IF $c { RETURN 1; } ELSE { RETURN 's'; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];
        let if_statement = source_output
            .statements
            .iter()
            .find(|statement| statement.kind == "if_else")
            .expect("if/else statement exists");

        assert_eq!(
            if_statement.response_kind,
            Some(Kind::Either(vec![Kind::Int, Kind::String,]))
        );
        assert_eq!(source_output.response_kind, if_statement.response_kind);
    }

    #[test]
    fn analyze_workspace_collapses_matching_if_else_return_shapes() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "IF true { RETURN 1; } ELSE { RETURN 2; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let if_statement = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "if_else")
            .expect("if/else statement exists");

        assert_eq!(if_statement.response_kind, Some(Kind::Int));
    }

    #[test]
    fn analyze_workspace_uses_prior_let_variables_in_if_else_return_shapes() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $age = 42;\nIF true { RETURN $age; } ELSE { RETURN $age + 1; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];
        let if_statement = source_output
            .statements
            .iter()
            .find(|statement| statement.kind == "if_else")
            .expect("if/else statement exists");

        assert!(source_output.inferred_params.is_empty());
        assert_eq!(if_statement.response_kind, Some(Kind::Int));
    }

    #[test]
    fn analyze_workspace_reports_non_bool_if_condition_literals() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "IF 1 { RETURN 1; } ELSE { RETURN 2; };\nIF 'yes' { RETURN 1; } ELSE { RETURN 2; };"
                .into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E2005" && message == "this IF condition is a `int`, not a `bool`"
        }));
        assert!(messages.iter().any(|(code, message)| {
            code == "E2005" && message == "this IF condition is a `string`, not a `bool`"
        }));
    }

    #[test]
    fn analyze_workspace_allows_bool_and_dynamic_if_conditions() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $flag = true;\nIF $flag { RETURN 1; } ELSE { RETURN 2; };\nIF $runtime { RETURN 1; } ELSE { RETURN 2; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];

        assert!(source_output
            .diagnostics
            .iter()
            .all(|finding| finding.code().to_string() != "E2005"));
        assert_eq!(source_output.inferred_params.len(), 1);
        assert_eq!(source_output.inferred_params[0].name, "runtime");
        assert_eq!(source_output.inferred_params[0].kind, None);
    }

    #[test]
    fn analyze_workspace_reports_non_bool_if_condition_from_let() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $flag = 1;\nIF $flag { RETURN 1; } ELSE { RETURN 2; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E2005" && message == "this IF condition is a `int`, not a `bool`"
        }));
    }

    #[test]
    fn analyze_workspace_reports_non_bool_if_condition_from_branch_local_let() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "IF true { LET $flag = 1; IF $flag { RETURN 1; } ELSE { RETURN 2; }; } ELSE { RETURN 3; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E2005" && message == "this IF condition is a `int`, not a `bool`"
        }));
    }

    #[test]
    fn analyze_workspace_keeps_if_branch_let_variables_local() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "IF true { LET $branch = 1; } ELSE { LET $branch = 2; };\nRETURN $branch;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];
        let return_statement = source_output
            .statements
            .iter()
            .find(|statement| statement.kind == "return")
            .expect("return statement exists");

        assert_eq!(source_output.inferred_params.len(), 1);
        assert_eq!(source_output.inferred_params[0].name, "branch");
        assert_eq!(source_output.inferred_params[0].kind, None);
        assert!(matches!(return_statement.response_kind, Some(Kind::Any)));
    }

    #[test]
    fn analyze_workspace_resolves_if_branch_local_let_returns() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            // A dynamic guard keeps both branches reachable (a constant guard
            // would fold to a single branch); this exercises the value union.
            "IF $c { LET $branch = 1; RETURN $branch; } ELSE { LET $branch = 's'; RETURN $branch; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let if_statement = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "if_else")
            .expect("if statement exists");

        let Some(Kind::Either(variants)) = &if_statement.response_kind else {
            panic!(
                "expected IF branch either kind, got {:?}",
                if_statement.response_kind
            );
        };
        assert_eq!(variants.len(), 2);
        assert!(variants.contains(&Kind::Int));
        assert!(variants.contains(&Kind::String));
    }

    #[test]
    fn analyze_workspace_outer_let_is_visible_inside_if_branch() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $outer = 1;\nIF true { LET $branch = $outer + 1; RETURN $branch; } ELSE { RETURN $outer; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let if_statement = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "if_else")
            .expect("if statement exists");

        assert_eq!(if_statement.response_kind, Some(Kind::Int));
    }

    #[test]
    fn analyze_workspace_infers_param_kinds_from_function_signatures() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nSELECT string::len($name) AS name_len, array::len($tags) AS tag_count, count() AS total FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;

        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "name");
        assert_eq!(params[0].kind, Some(Kind::String));
        assert_eq!(params[1].name, "tags");
        assert_eq!(params[1].kind, Some(Kind::Array(Box::new(Kind::Any), None)));
    }

    #[test]
    fn analyze_workspace_skips_function_param_inference_for_unknown_and_wrong_arity_calls() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nSELECT unknown::fn($value), string::len($first, $second), count($bad) FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;

        // `$value` is context-bound (6005 territory), never a host param;
        // the other three stay unknown because their call sites teach
        // nothing (unknown function, wrong arity, count(any)).
        let names: Vec<_> = params.iter().map(|param| param.name.as_str()).collect();
        assert_eq!(names, vec!["bad", "first", "second"]);
        assert!(params.iter().all(|param| param.kind.is_none()));
    }

    #[test]
    fn analyze_workspace_infers_param_kind_from_select_where_field_comparison() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT * FROM person WHERE name = $name;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;

        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "name");
        assert_eq!(params[0].kind, Some(Kind::String));
    }

    #[test]
    fn analyze_workspace_infers_param_kinds_from_select_where_comparison_operators() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nSELECT * FROM person WHERE age > $min_age AND $max_age >= age AND name != $excluded_name;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;
        let param_kinds: Vec<_> = params
            .iter()
            .map(|param| (param.name.as_str(), param.kind.clone()))
            .collect();

        assert_eq!(
            param_kinds,
            vec![
                ("excluded_name", Some(Kind::String)),
                ("max_age", Some(Kind::Int)),
                ("min_age", Some(Kind::Int)),
            ]
        );
    }

    #[test]
    fn analyze_workspace_infers_select_comparison_and_boolean_projection_shapes() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD active ON person TYPE bool;\nSELECT age > 18 AS adult, active = true AS matches_active, true AND active AS visible FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(Kind::Array(element, _)) = &select.response_kind else {
            panic!("expected array kind, got {:?}", select.response_kind);
        };
        let Kind::Literal(KindLiteral::Object(fields)) = element.as_ref() else {
            panic!("expected object literal element, got {element:?}");
        };

        assert_eq!(fields["adult"], Kind::Bool);
        assert_eq!(fields["matches_active"], Kind::Bool);
        assert_eq!(fields["visible"], Kind::Bool);
    }

    #[test]
    fn analyze_workspace_infers_param_kinds_from_let_env_predicates() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD strength ON likes TYPE float;\nLET $min_age = 21;\nLET $edge_strength = 0.5;\nSELECT * FROM person WHERE $min_age <= $age_param;\nSELECT * FROM person->(likes WHERE $edge_strength >= $min_strength)->post;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;
        let param_kinds: Vec<_> = params
            .iter()
            .map(|param| (param.name.as_str(), param.kind.clone()))
            .collect();

        assert_eq!(
            param_kinds,
            vec![
                ("age_param", Some(Kind::Int)),
                ("min_strength", Some(Kind::Float)),
            ]
        );
    }

    #[test]
    fn analyze_workspace_infers_param_kinds_from_graph_local_predicates() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD created_at ON likes TYPE datetime;\nDEFINE FIELD strength ON likes TYPE float;\nSELECT * FROM person->(likes WHERE created_at > $since AND strength >= $min_strength)->post;\nSELECT * FROM person->likes[WHERE created_at <= $before]->post;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;
        let param_kinds: Vec<_> = params
            .iter()
            .map(|param| (param.name.as_str(), param.kind.clone()))
            .collect();

        assert_eq!(
            param_kinds,
            vec![
                ("before", Some(Kind::Datetime)),
                ("min_strength", Some(Kind::Float)),
                ("since", Some(Kind::Datetime)),
            ]
        );
    }

    #[test]
    fn syntax_error_statement_yields_no_typed_response_or_params() {
        // A statement that fails to parse lowers to `Statement::Partial`: it
        // reports its syntax diagnostic and contributes no typed response and
        // no inferred params. (Its well-formed siblings, if any, still
        // analyze — see the resilience tests below.)
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source("query".into(), "SELECT * FROM ;".into());

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];

        assert_eq!(source_output.diagnostics.len(), 1);
        assert!(source_output.response_kind.is_none());
        assert!(source_output
            .statements
            .iter()
            .all(|statement| statement.response_kind.is_none()));
        assert!(source_output.inferred_params.is_empty());
    }

    #[test]
    fn resilient_editor_facts_survive_a_syntax_error_sibling() {
        // A syntax error in one statement must not darken the well-formed ones:
        // the LETs before and after the broken statement still bind and type.
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $good_a = 1 + 2;\nSELECT name FROM ;\nLET $good_b = 3 + 4;\n".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];

        // The broken statement still reports its syntax diagnostic.
        assert!(source_output
            .diagnostics
            .iter()
            .any(|finding| finding.code().number() == 1));
        // Both good LETs bind to `int`; the broken one contributes nothing.
        let bound: std::collections::BTreeMap<&str, Option<&Kind>> = source_output
            .let_bindings
            .iter()
            .map(|binding| (binding.name.as_str(), binding.kind.as_ref()))
            .collect();
        assert_eq!(bound.get("good_a"), Some(&Some(&Kind::Int)));
        assert_eq!(bound.get("good_b"), Some(&Some(&Kind::Int)));
    }

    #[test]
    fn resilient_editor_facts_survive_a_syntax_error_in_a_function_body() {
        // A syntax error inside a DEFINE FUNCTION body must not darken the rest
        // of the body: the LETs around the broken statement still bind and type,
        // and hover/inlay resolve against them.
        use crate::query::{hover_at, let_binding_hints};
        let mut workspace = Workspace::default();
        let text = "DEFINE FUNCTION fn::demo($p: int) {\n\
            LET $good_a = 1 + 2;\n\
            LET $bad = SELECT name FROM ;\n\
            LET $good_b = $p + 1;\n\
            RETURN $good_a;\n\
        };";
        let source = workspace.add_virtual_source("query".into(), text.into());
        let analysis = analyze_workspace(&workspace);
        let source_output = &analysis.sources[&source];

        // Syntax diagnostic still fires on the broken body statement.
        assert!(source_output
            .diagnostics
            .iter()
            .any(|finding| finding.code().number() == 1));

        let bound: std::collections::BTreeMap<&str, Option<&Kind>> = source_output
            .let_bindings
            .iter()
            .map(|binding| (binding.name.as_str(), binding.kind.as_ref()))
            .collect();
        // The good bindings survive; the broken `$bad` contributes nothing.
        assert_eq!(bound.get("good_a"), Some(&Some(&Kind::Int)));
        assert_eq!(bound.get("good_b"), Some(&Some(&Kind::Int)));
        assert!(bound.get("bad").is_none() || bound["bad"].is_none());

        // Inlay hints render for both good bindings.
        let hints = let_binding_hints(source_output);
        assert_eq!(hints.len(), 2);

        // Hover resolves on the good `$good_b` use in `RETURN`/its binding.
        let offset = text.rfind("$good_b").expect("has $good_b") as u32 + 1;
        let hover = hover_at(source_output, &analysis.schema, &source, text, offset)
            .expect("hover resolves on a good binding beside a broken sibling");
        assert!(hover.markdown.contains("int"));
    }

    #[test]
    fn resilient_go_to_definition_survives_a_syntax_error_sibling() {
        // Go-to-definition on a `$var` use still jumps to its binding even when
        // a sibling statement in the same body has a syntax error.
        use crate::query::definition_at;
        let mut workspace = Workspace::default();
        let text = "DEFINE FUNCTION fn::demo($p: int) {\n\
            LET $good_a = 1 + 2;\n\
            LET $bad = SELECT name FROM ;\n\
            RETURN $good_a;\n\
        };";
        let source = workspace.add_virtual_source("query".into(), text.into());
        let analysis = analyze_workspace(&workspace);
        let source_output = &analysis.sources[&source];

        // The `$good_a` in `RETURN $good_a` resolves to its `LET` binding site.
        let use_offset = text.rfind("$good_a").expect("has RETURN use") as u32 + 1;
        let target = definition_at(source_output, &analysis.schema, &source, text, use_offset)
            .expect("go-to-def resolves beside a broken sibling");
        let binding_offset = text.find("$good_a").expect("has binding") as u32;
        assert_eq!(target.span.range().start(), binding_offset);
    }

    #[test]
    fn analyze_workspace_allows_known_select_projection_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD profile.email ON person TYPE string;\nSELECT name, profile.email FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);

        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1002)));
    }

    #[test]
    fn analyze_workspace_reports_unknown_select_projection_fields() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT nickname, profile.phone FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_fields: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1002))
            .collect();

        assert_eq!(unknown_fields.len(), 2);
        assert_eq!(
            unknown_fields[0].message(),
            "`person` has no field `nickname`"
        );
        assert_eq!(unknown_fields[0].span().source(), &source);
        assert_eq!(unknown_fields[0].span().range().start(), 69);
        assert_eq!(unknown_fields[0].span().range().end(), 77);
        assert_eq!(
            unknown_fields[1].message(),
            "`person` has no field `profile.phone`"
        );
        assert_eq!(unknown_fields[1].span().range().start(), 79);
        assert_eq!(unknown_fields[1].span().range().end(), 92);
        // The two unknown fields (1002) plus the whole-table read lint (7014,
        // allow-by-default): this SELECT has no WHERE/LIMIT.
        assert_eq!(output.sources[&source].diagnostics.len(), 3);
    }

    #[test]
    fn analyze_workspace_reports_unknown_select_order_fields() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT * FROM person ORDER BY missing;".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_fields: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1002))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            unknown_fields,
            vec!["`person` has no field `missing`".to_string()]
        );
    }

    #[test]
    fn analyze_workspace_reports_unknown_select_group_and_split_fields() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT name FROM person GROUP BY missing_group SPLIT missing_split;".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_fields: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code(), code if code == FindingCode::schema(1002) || code == FindingCode::schema(1002)))
            .map(|finding| finding.message().to_string())
            .collect();
        let mut unknown_fields = unknown_fields;
        unknown_fields.sort();

        assert_eq!(
            unknown_fields,
            vec![
                "`person` has no field `missing_group`".to_string(),
                "`person` has no field `missing_split`".to_string(),
            ]
        );
    }

    #[test]
    fn analyze_workspace_reports_unknown_select_where_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT * FROM person WHERE missing = true AND name = 'Ada';".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1002))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(messages, vec!["`person` has no field `missing`"]);
    }

    #[test]
    fn analyze_workspace_reports_unknown_mutation_assignment_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nCREATE person SET nickname = 'Ada', name = 'Ada';\nUPDATE person SET handle = 'ada', name = 'Ada';\nUPSERT person SET alias = 'ada', name = 'Ada';".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1002))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "`person` has no field `nickname`",
                "`person` has no field `handle`",
                "`person` has no field `alias`",
            ]
        );
    }

    #[test]
    fn analyze_workspace_reports_unknown_object_mutation_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD profile.email ON person TYPE string;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD created_at ON likes TYPE datetime;\nCREATE person CONTENT { nickname: 'Ada', profile: { phone: '555' }, name: 'Ada' };\nINSERT INTO person { handle: 'ada', name: 'Ada' };\nINSERT INTO person (alias, name) VALUES ('ada', 'Ada');\nUPDATE person MERGE { stale: true, name: 'Ada' };\nUPSERT person REPLACE { missing: true, name: 'Ada' };\nRELATE person:one->likes->post:one CONTENT { missing_since: time::now(), created_at: time::now() };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code().number(), 1002..=1011))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "`person` has no field `nickname`",
                "`person` has no field `profile.phone`",
                "`person` has no field `handle`",
                "`person` has no field `alias`",
                "`person` has no field `stale`",
                "`person` has no field `missing`",
                "`likes` has no field `missing_since`",
            ]
        );
    }

    #[test]
    fn analyze_workspace_reports_mutation_value_type_mismatches() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nCREATE person SET age = 'old', name = 'Ada';\nUPDATE person MERGE { age: 'old', name: 'Ada' };\nUPSERT person CONTENT { age: 'old', name: 'Ada' };\nINSERT INTO person (age, name) VALUES ('old', 'Ada');".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code().number(), 2001..=2003))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "`age` is declared `int`, but this value is `string`",
                "`age` is declared `int`, but this value is `string`",
                "`age` is declared `int`, but this value is `string`",
                "`age` is declared `int`, but this value is `string`",
            ]
        );
    }

    #[test]
    fn analyze_workspace_skips_type_mismatch_for_dynamic_mutation_params() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nCREATE person SET age = $age;\nUPDATE person MERGE { age: $age };".into(),
        );

        let output = analyze_workspace(&workspace);
        let type_errors: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::type_error(2001))
            .collect();

        assert!(type_errors.is_empty());
    }

    #[test]
    fn analyze_workspace_reports_nested_and_relate_payload_type_mismatches() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD profile.email ON person TYPE string;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD created_at ON likes TYPE datetime;\nUPDATE person MERGE { profile: { email: 10 } };\nRELATE person:one->likes->post:one CONTENT { created_at: 'yesterday' };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::type_error(2001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "`profile.email` is declared `string`, but this value is `int`",
                "`created_at` is declared `datetime`, but this value is `string`",
            ]
        );
    }

    #[test]
    fn analyze_workspace_reports_tuple_insert_arity_mismatches() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nINSERT INTO person (age, name) VALUES (1);\nINSERT INTO person (age, name) VALUES (1, 'Ada', true);".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::statement(4004))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "this INSERT row has 1 value but 2 columns",
                "this INSERT row has 3 values but 2 columns",
            ]
        );
    }

    #[test]
    fn analyze_workspace_checks_tuple_insert_values_across_flattened_rows() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nINSERT INTO person (age, name) VALUES (1, 'Ada'), ('old', 'Grace');".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::type_error(2001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(messages, vec!["`age` is declared `int`, but this value is `string`"]);
    }

    #[test]
    fn analyze_workspace_reports_unknown_mutation_where_fields() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;\nUPDATE person SET name = 'Ada' WHERE missing > 0 AND age > 18;\nUPSERT person SET name = 'Ada' WHERE ghost = true AND name = 'Ada';\nDELETE person WHERE stale = true AND age < 99;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1002))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "`person` has no field `missing`",
                "`person` has no field `ghost`",
                "`person` has no field `stale`",
            ]
        );
        assert_eq!(output.sources[&source].diagnostics.len(), 3);
    }

    #[test]
    fn analyze_workspace_infers_param_kinds_from_mutation_where_comparisons() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nUPDATE person SET name = 'Ada' WHERE age > $min_age AND $max_age >= age;\nUPSERT person SET name = 'Ada' WHERE name != $excluded_name;\nDELETE person WHERE age <= $delete_before;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;
        let param_kinds: Vec<_> = params
            .iter()
            .map(|param| (param.name.as_str(), param.kind.clone()))
            .collect();

        assert_eq!(
            param_kinds,
            vec![
                ("delete_before", Some(Kind::Int)),
                ("excluded_name", Some(Kind::String)),
                ("max_age", Some(Kind::Int)),
                ("min_age", Some(Kind::Int)),
            ]
        );
    }

    #[test]
    fn analyze_workspace_validates_aliased_select_projection_source_field() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT nickname AS display_name FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_fields: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1002))
            .collect();

        assert_eq!(unknown_fields.len(), 1);
        assert_eq!(
            unknown_fields[0].message(),
            "`person` has no field `nickname`"
        );
    }

    #[test]
    fn analyze_workspace_skips_wildcard_select_projection_field_validation() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nSELECT * FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);

        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1002)));
    }

    #[test]
    fn analyze_workspace_validates_and_shapes_parent_object_paths() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD profile.name ON person TYPE string;\nSELECT profile FROM person WHERE profile = $profile;".into(),
        );

        let output = analyze_workspace(&workspace);
        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::schema(1002)));
        let params = &output.sources[&source].inferred_params;
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "profile");
        // The comparison against the declared object field constrains the
        // parameter to that field's shape.
        assert_eq!(
            params[0].kind,
            Some(Kind::Literal(surrealdb_types::KindLiteral::Object(
                std::collections::BTreeMap::from([("name".to_string(), Kind::String)])
            )))
        );

        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");
        let Some(Kind::Array(element, _)) = &select.response_kind else {
            panic!("expected array kind, got {:?}", select.response_kind);
        };
        let Kind::Literal(KindLiteral::Object(fields)) = element.as_ref() else {
            panic!("expected object literal element, got {element:?}");
        };
        let Kind::Literal(KindLiteral::Object(profile_fields)) = &fields["profile"] else {
            panic!(
                "expected nested object literal, got {:?}",
                fields["profile"]
            );
        };
        assert_eq!(profile_fields["name"], Kind::String);
    }

    #[test]
    fn analyze_workspace_infers_row_brace_selector_without_alias_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD profile.name ON person TYPE string;\nDEFINE FIELD profile.age ON person TYPE int;\nDEFINE FIELD profile.secret ON person TYPE string;\nSELECT profile.{name, age} FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(Kind::Array(element, _)) = &select.response_kind else {
            panic!("expected array kind, got {:?}", select.response_kind);
        };
        let Kind::Literal(KindLiteral::Object(fields)) = element.as_ref() else {
            panic!("expected object literal element, got {element:?}");
        };
        let Kind::Literal(KindLiteral::Object(profile_fields)) = &fields["profile"] else {
            panic!(
                "expected nested object literal, got {:?}",
                fields["profile"]
            );
        };

        assert_eq!(
            profile_fields
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["age", "name"]
        );
        assert_eq!(profile_fields["name"], Kind::String);
        assert_eq!(profile_fields["age"], Kind::Int);
    }

    #[test]
    fn analyze_workspace_infers_row_brace_selector_alias_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD profile.name ON person TYPE string;\nDEFINE FIELD profile.age ON person TYPE int;\nSELECT profile.{name, age} AS public_profile FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(Kind::Array(element, _)) = &select.response_kind else {
            panic!("expected array kind, got {:?}", select.response_kind);
        };
        let Kind::Literal(KindLiteral::Object(fields)) = element.as_ref() else {
            panic!("expected object literal element, got {element:?}");
        };
        let Kind::Literal(KindLiteral::Object(profile_fields)) = &fields["public_profile"] else {
            panic!(
                "expected nested object literal, got {:?}",
                fields["public_profile"]
            );
        };

        assert_eq!(profile_fields["name"], Kind::String);
        assert_eq!(profile_fields["age"], Kind::Int);
    }

    #[test]
    fn analyze_workspace_reports_binary_expression_mismatch_from_let_env() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "LET $age = 42;\nLET $bad = $age + 'x';\nRETURN $age + 'x';\nIF true { LET $score = 7; RETURN $score + 'x'; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        let binary_mismatches = messages
            .iter()
            .filter(|(code, message)| {
                code == "E2004" && message == "`+` can't combine a `int` and a `string`"
            })
            .count();
        assert_eq!(binary_mismatches, 3);
    }

    #[test]
    fn analyze_workspace_accepts_temporal_and_collection_arithmetic() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE event;\nDEFINE FIELD at ON event TYPE datetime;\nDEFINE FIELD took ON event TYPE duration;\nDEFINE FIELD tags ON event TYPE array<string>;\nSELECT at + 1w AS soon, at - at AS gap, took * 2 AS twice, tags + ['x'] AS more FROM event WHERE at - took < time::now();".into(),
        );

        let output = analyze_workspace(&workspace);
        let operand_findings: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::type_error(2004))
            .collect();

        // Valid SurrealQL temporal/collection arithmetic must not be flagged.
        assert_eq!(operand_findings, Vec::<&Finding>::new());
    }

    #[test]
    fn analyze_workspace_reports_none_assignment_and_bad_negation() {
        // (2014 negation coverage waits on the grammar: prefix `-` only
        // parses on numeric literals today.)
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD retired_at ON person TYPE option<datetime>;\nUPDATE person SET age = NONE, retired_at = NONE;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        // NONE into a non-optional field violates the one assignability
        // contract (2001); into option<datetime> it is fine.
        assert!(messages.iter().any(|(code, message)| {
            code == "E2001"
                && message == "`age` is not optional, so it can't be set to none"
        }));
        assert!(!messages
            .iter()
            .any(|(_, message)| message.contains("retired_at")));
    }

    #[test]
    fn analyze_workspace_reports_binary_expression_type_mismatches() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD name ON person TYPE string;\nSELECT age + name AS bad FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];
        let messages: Vec<_> = source_output
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E2004" && message == "`+` can't combine a `int` and a `string`"
        }));
    }

    #[test]
    fn analyze_workspace_infers_extended_verified_function_projection_shapes_and_params() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD title ON person TYPE string;\nDEFINE FIELD tags ON person TYPE array;\nSELECT string::lowercase(name) AS lower, string::uppercase(name) AS upper, string::contains(name, 'a') AS has_needle, string::starts_with(name, 'a') AS starts, string::ends_with(name, 'z') AS ends, array::is_empty(tags) AS no_items FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(Kind::Array(element, _)) = &select.response_kind else {
            panic!("expected array kind, got {:?}", select.response_kind);
        };
        let Kind::Literal(KindLiteral::Object(fields)) = element.as_ref() else {
            panic!("expected object literal element, got {element:?}");
        };
        assert_eq!(fields["lower"], Kind::String);
        assert_eq!(fields["upper"], Kind::String);
        assert_eq!(fields["has_needle"], Kind::Bool);
        assert_eq!(fields["starts"], Kind::Bool);
        assert_eq!(fields["ends"], Kind::Bool);
        assert_eq!(fields["no_items"], Kind::Bool);
    }

    #[test]
    fn analyze_workspace_indexes_define_param_defaults_for_later_references() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE PARAM $age VALUE 42;\nSELECT * FROM person WHERE age = $age;".into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;

        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "age");
        assert_eq!(params[0].kind, Some(Kind::Int));
        assert!(!params[0].required);
    }

    #[test]
    fn analyze_workspace_uses_define_param_default_for_function_diagnostics() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE PARAM $name VALUE 'Ada';\nSELECT string::len($name) AS name_len FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(
            messages.iter().all(|(code, _)| code != "E2004"),
            "defined string param should satisfy string::len: {messages:?}"
        );
    }

    #[test]
    fn analyze_workspace_infers_params_from_extended_verified_function_signatures() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nSELECT string::lowercase($upper), string::contains($haystack, $needle), array::is_empty($items) FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let param_kinds: Vec<_> = output.sources[&source]
            .inferred_params
            .iter()
            .map(|param| (param.name.as_str(), param.kind.clone()))
            .collect();

        assert_eq!(
            param_kinds,
            vec![
                ("haystack", Some(Kind::String)),
                ("items", Some(Kind::Array(Box::new(Kind::Any), None))),
                ("needle", Some(Kind::String)),
                ("upper", Some(Kind::String)),
            ]
        );
    }

    #[test]
    fn analyze_workspace_reports_function_argument_mismatch_from_branch_local_let() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nIF true { LET $age = 42; SELECT string::len($age) FROM person; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E5002" && message == "argument 1 to `string::len` is a `int`, but `string` is required"
        }));
    }

    #[test]
    fn analyze_workspace_reports_function_mismatch_in_mutation_return_expression() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $age = 42;\nUPDATE person SET age = $age RETURN string::len($age) AS bad_len;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E5002" && message == "argument 1 to `string::len` is a `int`, but `string` is required"
        }));
    }

    #[test]
    fn analyze_workspace_reports_function_misuse_in_where_and_set_positions() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nSELECT * FROM person WHERE string::len(age) > 0;\nUPDATE person SET age = math::abs('x');".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        // WHERE conditions and SET values are walked like any other
        // expression position.
        assert!(messages.iter().any(|(code, message)| {
            code == "E5002" && message == "argument 1 to `string::len` is a `int`, but `string` is required"
        }));
        assert!(messages.iter().any(|(code, message)| {
            code == "E5002" && message == "argument 1 to `math::abs` is a `string`, but a number is required"
        }));
    }

    #[test]
    fn analyze_workspace_reports_unknown_custom_functions() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE FUNCTION fn::greet($name: string) -> string { RETURN 'hi'; };\nRETURN fn::greet('a');\nRETURN fn::gret('a');".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E5001" && message == "`fn::gret` is not a defined function"
        }));
        // The defined function produces no finding.
        assert!(!messages
            .iter()
            .any(|(_, message)| message.contains("fn::greet")));
    }

    #[test]
    fn analyze_workspace_reports_function_arity_and_argument_kind_mismatches() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nSELECT string::len(), string::len(age), array::len(age), unknown::fn(age) FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let source_output = &output.sources[&source];
        let messages: Vec<_> = source_output
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        assert!(messages.iter().any(|(code, message)| {
            code == "E5002" && message == "`string::len` takes 1 argument, but this call passes 0"
        }));
        assert!(messages.iter().any(|(code, message)| {
            code == "E5002" && message == "argument 1 to `string::len` is a `int`, but `string` is required"
        }));
        assert!(messages.iter().any(|(code, message)| {
            code == "E5002" && message == "argument 1 to `array::len` is a `int`, but an array is required"
        }));
        assert!(messages.iter().any(|(code, message)| {
            code == "E5001" && message == "`unknown::fn` is not a known function"
        }));
    }

    #[test]
    fn analyze_workspace_infers_select_value_expression_response_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nSELECT VALUE age + 1 FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(Kind::Array(element, _)) = &select.response_kind else {
            panic!("expected array kind, got {:?}", select.response_kind);
        };
        assert_eq!(element.as_ref(), &Kind::Int);
    }

    #[test]
    fn analyze_workspace_infers_select_value_function_response_shape() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT VALUE string::lowercase(name) FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let Some(Kind::Array(element, _)) = &select.response_kind else {
            panic!("expected array kind, got {:?}", select.response_kind);
        };
        assert_eq!(element.as_ref(), &Kind::String);
    }

    #[test]
    fn analyze_workspace_reports_unknown_mutation_return_fields() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nUPDATE person RETURN nickname, profile.phone;".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_fields: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1002))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            unknown_fields,
            vec![
                "`person` has no field `nickname`".to_string(),
                "`person` has no field `profile.phone`".to_string(),
            ]
        );
    }

    #[test]
    fn analyze_workspace_validates_mutation_return_alias_expressions_without_alias_field_false_positive(
    ) {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nUPDATE person RETURN name AS label, missing AS projected_missing;".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_fields: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1002))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            unknown_fields,
            vec!["`person` has no field `missing`".to_string()]
        );
    }

    #[test]
    fn analyze_workspace_infers_mutation_return_expression_shape_from_let_env() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nLET $bonus = 1;\nUPDATE person SET age = 42 RETURN $bonus + 1 AS next_bonus;".into(),
        );

        let output = analyze_workspace(&workspace);
        let update = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "update")
            .expect("update statement exists");

        let Some(Kind::Array(element, _)) = &update.response_kind else {
            panic!("expected array kind, got {:?}", update.response_kind);
        };
        let Kind::Literal(KindLiteral::Object(fields)) = element.as_ref() else {
            panic!("expected object literal element, got {element:?}");
        };
        assert_eq!(
            fields.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["next_bonus"]
        );
        assert_eq!(fields["next_bonus"], Kind::Int);
    }

    #[test]
    fn analyze_workspace_validates_omit_and_fetch_field_paths() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT * OMIT password FROM person FETCH friend;".into(),
        );

        let output = analyze_workspace(&workspace);
        let unknown_fields: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code(), code if code == FindingCode::schema(1002) || code == FindingCode::schema(1002)))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            unknown_fields,
            vec![
                "`person` has no field `password`",
                "`person` has no field `friend`",
            ]
        );
    }

    #[test]
    fn analyze_workspace_exposes_row_preserving_select_modifier_facts() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nSELECT name FROM person WHERE name = 'Ada' ORDER BY name LIMIT 5 START 2 TIMEOUT 1s PARALLEL;".into(),
        );

        let output = analyze_workspace(&workspace);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        let modifiers: Vec<_> = select
            .select_modifiers
            .iter()
            .map(|modifier| {
                (
                    modifier.kind.as_str(),
                    modifier.row_preserving,
                    modifier.max_len,
                )
            })
            .collect();

        assert_eq!(
            modifiers,
            vec![
                ("where", true, None),
                ("order", true, None),
                ("limit", true, Some(5)),
                ("start", true, None),
                ("timeout", true, None),
                ("parallel", true, None),
            ]
        );
    }

    #[test]
    fn analyze_workspace_reports_unknown_later_multi_hop_graph_edge_table() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE comment;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT * FROM person->likes->post->missing->comment;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(messages, vec!["`missing` is not a defined table"]);
    }

    #[test]
    fn analyze_workspace_reports_unknown_graph_edge_and_target_tables() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT * FROM person->missing->post;\nSELECT * FROM person->likes->ghost;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec!["`missing` is not a defined table", "`ghost` is not a defined table",]
        );
    }

    #[test]
    fn analyze_workspace_reports_unknown_parenthesized_graph_edge_table() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nSELECT * FROM person->(missing WHERE created_at > $since)->post;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(messages, vec!["`missing` is not a defined table"]);
    }

    #[test]
    fn analyze_workspace_reports_clause_value_and_lint_findings() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nDEFINE FIELD tags ON person TYPE array<string>;\nLET $lim = 'a';\nSELECT * FROM person LIMIT $lim;\nSELECT * FROM person START -1;\nSELECT * FROM person FETCH age;\nSELECT * FROM person SPLIT age;\nSELECT age FROM person ORDER BY name;\nDEFINE FIELD name ON person TYPE string;\nLET $auth = 1;\nRETURN [1, 'a'];\nIF true { RETURN 1; };\nRETURN array::map([1], |$v, $i, $extra| $v);\nSELECT type::field('ghost') FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        for (code, message) in [
            ("E2018", "LIMIT needs an integer, but this is a `string`"),
            ("E2018", "START can't be negative"),
            ("E1023", "FETCH `age` does nothing — `int` holds no records"),
            (
                "E1024",
                "SPLIT needs a collection field, but `age` is a `int`",
            ),
            (
                "E2017",
                "ORDER BY `name` doesn't name a field of this query's rows",
            ),
            (
                "E6007",
                "`$auth` is a protected parameter and can't be assigned",
            ),
            ("L7003", "array literal mixes kinds: `int`, `string`"),
            ("L7004", "this IF condition is constant, so one branch is never taken"),
            (
                "E5002",
                "`array::map` calls its closure with 2 arguments; `$extra` is never bound",
            ),
            ("E5005", "`ghost` is not a field of table `person`"),
        ] {
            assert!(
                messages.iter().any(|(c, m)| c == code && m == message),
                "missing {code}: {message}\nhave: {messages:#?}"
            );
        }
    }

    #[test]
    fn analyze_workspace_exports_param_constraints() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            concat!(
                "DEFINE TABLE person SCHEMAFULL;\n",
                "DEFINE FIELD age ON person TYPE int DEFAULT 0;\n",
                "DEFINE FIELD name ON person TYPE string DEFAULT '';\n",
                "UPDATE person SET age = $age WHERE name = $who;\n",
                "SELECT * FROM person LIMIT $page_size;\n",
                "SELECT type::field($field) FROM person;\n",
                "SELECT * FROM person WHERE age > $min AND $min = 'x';\n",
            )
            .into(),
        );

        let output = analyze_workspace(&workspace);
        let params = &output.sources[&source].inferred_params;
        let get = |name: &str| params.iter().find(|p| p.name == name).unwrap();

        // Assignment position: the field's kind.
        assert_eq!(get("age").kind, Some(Kind::Int));
        // Comparison position: the other side's kind.
        assert_eq!(get("who").kind, Some(Kind::String));
        // LIMIT: an integer with a non-negative domain.
        assert_eq!(get("page_size").kind, Some(Kind::Int));
        assert_eq!(
            get("page_size").domain,
            Some(ValueDomain::Range {
                min: Some(0),
                max: None
            })
        );
        // Value-dependent builtin: string plus the field-path domain.
        assert_eq!(get("field").kind, Some(Kind::String));
        let Some(ValueDomain::OneOf(paths)) = &get("field").domain else {
            panic!("expected OneOf domain, got {:?}", get("field").domain);
        };
        assert!(paths.contains(&surrealdb_types::Value::String("age".into())));
        // Irreconcilable uses: int vs string on $min is 6001.
        assert!(output.sources[&source].diagnostics.iter().any(|finding| {
            finding.code().to_string() == "E6001"
                && finding.message().contains("cannot satisfy this query")
        }));
    }

    #[test]
    fn analyze_workspace_param_compared_to_two_record_tables_is_not_6001() {
        // A host param compared against two different edges' record fields
        // (`in = $p`, then `out = $p`) is satisfiable — records of different
        // tables compare fine, they just aren't equal. No 6001; the param
        // widens to the union of the two record targets.
        let mut workspace = Workspace::default();
        let _source = workspace.add_virtual_source(
            "query".into(),
            concat!(
                "DEFINE TABLE account SCHEMAFULL;\n",
                "DEFINE TABLE team SCHEMAFULL;\n",
                "DEFINE TABLE membership TYPE RELATION IN account OUT team SCHEMAFULL;\n",
                "SELECT * FROM membership WHERE in = $p;\n",
                "SELECT * FROM membership WHERE out = $p;\n",
            )
            .into(),
        );

        let output = analyze_workspace(&workspace);
        assert_eq!(
            codes(&output, 6001),
            0,
            "records of different tables compare fine: {:?}",
            output.diagnostics
        );
    }

    #[test]
    fn analyze_workspace_session_param_guard_then_edge_compare_is_not_6001() {
        // The workshop pattern: `IF $auth = NONE ...` (a `none` comparison)
        // then `WHERE in = $auth` (a record comparison). `$auth` is a session
        // param and the NONE guard is an existence check — not a demand that it
        // be `none`. No 6001.
        let mut workspace = Workspace::default();
        let _source = workspace.add_virtual_source(
            "query".into(),
            concat!(
                "DEFINE TABLE account SCHEMAFULL;\n",
                "DEFINE TABLE org SCHEMAFULL;\n",
                "DEFINE TABLE employee_of TYPE RELATION IN account OUT org SCHEMAFULL;\n",
                "DEFINE FUNCTION fn::guard() {\n",
                "  IF $auth = NONE THEN RETURN false END;\n",
                "  LET $rows = SELECT VALUE id FROM employee_of WHERE in = $auth;\n",
                "  RETURN array::len($rows) > 0;\n",
                "};\n",
            )
            .into(),
        );

        let output = analyze_workspace(&workspace);
        assert_eq!(
            codes(&output, 6001),
            0,
            "session-param NONE guard then edge compare is satisfiable: {:?}",
            output.diagnostics
        );
    }

    #[test]
    fn analyze_workspace_either_of_arrays_satisfies_array_argument() {
        // `$rows ?? []` infers `array<...> | array<any, 0>` — a union whose
        // every variant is an array, so it satisfies `array::concat`'s array
        // parameter. No 5002.
        let mut workspace = Workspace::default();
        let _source = workspace.add_virtual_source(
            "query".into(),
            concat!(
                "DEFINE TABLE org SCHEMAFULL;\n",
                "DEFINE FUNCTION fn::principals() {\n",
                "  LET $orgs = SELECT VALUE id FROM org;\n",
                "  RETURN array::concat([], $orgs ?? []);\n",
                "};\n",
            )
            .into(),
        );

        let output = analyze_workspace(&workspace);
        assert_eq!(
            codes(&output, 5002),
            0,
            "an array-or-empty-array union is an array: {:?}",
            output.diagnostics
        );
    }

    #[test]
    fn analyze_workspace_machinery_batch() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            concat!(
                "DEFINE TABLE person SCHEMAFULL;\n",
                "DEFINE FIELD age ON person TYPE int DEFAULT 0;\n",
                "DEFINE FUNCTION fn::bad() -> string { RETURN 1; };\n",
                "DEFINE FUNCTION fn::loop_a() { RETURN fn::loop_b(); };\n",
                "DEFINE FUNCTION fn::loop_b() { RETURN fn::loop_a(); };\n",
                "DEFINE EVENT audit ON person WHEN $event = 'CRATE' THEN { RETURN 1; };\n",
                "RETURN $before;\n",
                "COMMIT;\n",
                "BEGIN;\n",
                "BEGIN;\n",
                "COMMIT;\n",
                "BEGIN;\n",
                "RETURN 1;\n",
            )
            .into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        for (code, needle) in [
            (
                "E2012",
                "`fn::bad` declares `-> string` but its body returns `int`",
            ),
            ("E5009", "never terminates"),
            // $event is the literal union, so the typo comparison is the
            // ordinary always-false lint with the edge-filter machinery.
            ("L7005", "`=` between"),
            (
                "E6005",
                "`$before` only exists inside the construct that binds it",
            ),
            ("E4007", "COMMIT/CANCEL without an open BEGIN"),
            ("E4007", "BEGIN inside an open transaction"),
            ("E4007", "this BEGIN is never closed"),
        ] {
            assert!(
                messages
                    .iter()
                    .any(|(c, m)| c == code && m.contains(needle)),
                "missing {code}: {needle}\nhave: {messages:#?}"
            );
        }
    }

    #[test]
    fn analyze_workspace_full_coverage_batch_two() {
        // Slice F: literal content (2032/2031), casts (2008), SINCE (2021),
        // FOR iterables (2022 via params; literals are parser-rejected),
        // PATCH shapes (2033), GeoJSON (2036), FROM landings (3004),
        // non-record traversal starts (3009), index-backed operators
        // (1027), duplicate indexes (1029), analyzer components
        // (1032/2035), recursion bounds (3011), use-before-LET (6004).
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            concat!(
                "DEFINE TABLE person SCHEMAFULL;\n",
                "DEFINE FIELD name ON person TYPE string DEFAULT 'x';\n",
                "DEFINE FIELD age ON person TYPE int DEFAULT 0;\n",
                "DEFINE TABLE post;\n",
                "DEFINE TABLE likes TYPE RELATION IN person OUT post;\n",
                "DEFINE INDEX by_name ON person FIELDS name;\n",
                "DEFINE INDEX by_name_too ON person FIELDS name;\n",
                "DEFINE ANALYZER myan TOKENIZERS blank FILTERS snowball(klingon),edgengram(9,2);\n",
                "RETURN d'2024-13-45T00:00:00Z';\n",
                "RETURN 'a' ~ 'unclosed(';\n",
                "RETURN <int> 'abc';\n",
                "RETURN <duration> true;\n",
                "SHOW CHANGES FOR TABLE person SINCE 'not-a-date';\n",
                "LET $n = 42;\n",
                "FOR $x IN $n { RETURN 1; };\n",
                "UPDATE person PATCH [{ op: 'remvoe', path: 'name' }] WHERE name = 'x';\n",
                "RETURN { type: 'Pointt', coordinates: [1, 2] };\n",
                "SELECT * FROM person->likes;\n",
                "SELECT age->likes->post FROM person;\n",
                "SELECT * FROM person WHERE name @@ 'q';\n",
                "SELECT ->likes.{..}->post FROM person;\n",
                "RETURN $later;\n",
                "LET $later = 1;\n",
            )
            .into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        for (code, needle) in [
            ("E2032", "not a valid datetime"),
            ("E2031", "not a valid regex"),
            ("E2008", "`abc` can't be cast to `int`"),
            ("E2008", "a `bool` can't be cast to `duration`"),
            ("E2021", "needs a versionstamp or datetime"),
            ("E2022", "FOR can't iterate a `int`"),
            ("E2033", "`remvoe` is not a PATCH operation"),
            ("E2033", "PATCH paths start with `/`"),
            ("E2036", "`Pointt` is not a GeoJSON geometry type"),
            ("E3004", "not on a table"),
            ("E3009", "can't start from `age`"),
            ("E1027", "needs a SEARCH ANALYZER index"),
            ("E1029", "covers the same fields as `by_name`"),
            ("E1032", "`klingon` is not a supported snowball language"),
            ("E2035", "needs `(min, max)` with min <= max"),
            ("E3011", "no upper bound"),
            ("E6004", "read before its LET"),
        ] {
            assert!(
                messages
                    .iter()
                    .any(|(c, m)| c == code && m.contains(needle)),
                "missing {code}: {needle}\nhave: {messages:#?}"
            );
        }
    }

    #[test]
    fn analyze_workspace_checks_define_field_clauses() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            concat!(
                "DEFINE TABLE person SCHEMAFULL;\n",
                "DEFINE FIELD age ON person TYPE int DEFAULT 'young';\n",
                "DEFINE FIELD score ON person TYPE int ASSERT $value + 1;\n",
                "DEFINE FIELD ratio ON person TYPE int ASSERT $value > 'high';\n",
                "DEFINE FIELD created ON person TYPE datetime VALUE time::now() READONLY;\n",
                "DEFINE FIELD synced ON person TYPE bool VALUE http::get('https://x.test');\n",
                "UPDATE person SET created = time::now();\n",
                "UPDATE person SET synced = true;\n",
            )
            .into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        for (code, message) in [
            // DEFAULT must inhabit the declared type.
            ("E2001", "`age`'s value is `string`, but the field is declared `int`"),
            // ASSERT is a condition; `$value` carries the declared kind.
            ("E2005", "this ASSERT is a `int`, not a `bool`"),
            ("E2004", "`>` can't combine a `int` and a `string`"),
            // READONLY blocks non-creation writes; computed fields warn.
            ("E2025", "`created` can't be changed after creation"),
            (
                "E2026",
                "this write to `synced` is discarded",
            ),
            (
                "L7012",
                "`http::get` runs on every write to this row",
            ),
        ] {
            assert!(
                messages.iter().any(|(c, m)| c == code && m == message),
                "missing {code}: {message}\nhave: {messages:#?}"
            );
        }
    }

    #[test]
    fn analyze_workspace_full_coverage_batch_one() {
        // Slice batch: required fields (2034), relation writes (4019),
        // RETURN BEFORE on CREATE (4020), whole-table writes (7009),
        // SET id (7011), compound assignment operands (2004), fn::
        // signatures (5002), loop contracts (4005), unreachable code
        // (4006), shadowing (7002), wildcard-plus-field (7007),
        // OMIT-without-* (4012), read-position writes (4018), empty
        // membership (7006), DROP reads (4022), changefeed (4021).
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            concat!(
                "DEFINE TABLE person SCHEMAFULL;\n",
                "DEFINE FIELD name ON person TYPE string;\n",
                "DEFINE FIELD age ON person TYPE int DEFAULT 0;\n",
                "DEFINE TABLE post;\n",
                "DEFINE TABLE likes TYPE RELATION IN person OUT post;\n",
                "DEFINE TABLE audit DROP;\n",
                "DEFINE FUNCTION fn::greet($who: string) -> string { RETURN 'hi'; };\n",
                "CREATE person;\n",
                "CREATE person RETURN BEFORE;\n",
                "CREATE likes SET strength = 1;\n",
                "UPDATE person SET name = 'Ada';\n",
                "UPDATE person SET id = person:two WHERE name = 'Ada';\n",
                "UPDATE person SET age += '1' WHERE name = 'Ada';\n",
                "RETURN fn::greet(1);\n",
                "RETURN fn::greet();\n",
                "BREAK;\n",
                "RETURN { LET $x = 1; RETURN $x; LET $y = 2; };\n",
                "LET $shadow = 1;\n",
                "IF $shadow > 0 { LET $shadow = 2; RETURN $shadow; };\n",
                "SELECT *, name FROM person;\n",
                "SELECT (CREATE person SET name = 'x') AS made FROM person;\n",
                "SELECT * FROM person WHERE name IN [];\n",
                "SELECT * FROM audit;\n",
                "SHOW CHANGES FOR TABLE person SINCE 0;\n",
            )
            .into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        for (code, message) in [
            ("E2034", "`name` must be set when creating a `person`"),
            (
                "E4020",
                "RETURN BEFORE on CREATE is always NONE; there is no before state",
            ),
            (
                "E4019",
                "`likes` is a relation; use RELATE (or provide `in` and `out`)",
            ),
            (
                "L7009",
                "this writes every row of `person`; add WHERE or a record id",
            ),
            ("L7011", "record ids are immutable; `id` is set at creation"),
            (
                "E2004",
                "`+=` can't combine a `int` and a `string`",
            ),
            (
                "E5002",
                "argument 1 to `fn::greet` is a `int`, but `$who` is declared `string`",
            ),
            ("E5002", "`fn::greet` takes 1 argument, but this call passes 0"),
            ("E4005", "BREAK here does nothing — it is outside any FOR loop"),
            ("E4006", "this statement is unreachable — the block already returned"),
            (
                "L7002",
                "`$shadow` is re-bound inside this block; the outer `$shadow` is unchanged",
            ),
            ("L7007", "this field is already included by `*`"),
            (
                "E4018",
                "this SELECT hides a write; run the mutation as its own statement",
            ),
            (
                "L7006",
                "membership test against an empty collection is always false",
            ),
            ("E4022", "`audit` is a DROP table, so this SELECT never returns rows"),
            (
                "E4021",
                "`person` has no CHANGEFEED, so SHOW CHANGES reads nothing",
            ),
        ] {
            assert!(
                messages.iter().any(|(c, m)| c == code && m == message),
                "missing {code}: {message}\nhave: {messages:#?}"
            );
        }
    }

    #[test]
    fn analyze_workspace_reports_statement_shape_misuse() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nSELECT * FROM ONLY person;\nSELECT * FROM ONLY person LIMIT 1;\nSELECT * FROM ONLY person:one;\nUPDATE ONLY person SET age = 1;\nUPDATE person SET age = 1, age = 2;\nSELECT age, age FROM person;\nLET $y = { LET $inner = 1; };".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code().number(), 4001..=4020))
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        let expect = [
            (
                "E4003",
                "ONLY needs a single-row target, but this reads a whole table",
            ),
            ("E4003", "ONLY on a whole table needs a record id target"),
            (
                "W4010",
                "`age` is assigned more than once; the last assignment wins",
            ),
            ("W4011", "`age` is projected twice; the later one wins"),
            (
                "W4017",
                "block ends with LET, so its value is NONE — return the value instead",
            ),
        ];
        for (code, message) in expect {
            // Rendered test codes carry the category letter; severities are
            // asserted through the severity() accessor below.
            assert!(
                messages
                    .iter()
                    .any(|(c, m)| c.trim_start_matches(char::is_alphabetic)
                        == code.trim_start_matches(char::is_alphabetic)
                        && m == message),
                "missing {code}: {message} in {messages:?}"
            );
        }
        // The guarded forms produce nothing: LIMIT 1 and record ids are
        // exactly the escape hatches.
        assert_eq!(
            messages.iter().filter(|(_, m)| m.contains("ONLY")).count(),
            2
        );
    }

    #[test]
    fn analyze_workspace_type_checks_edge_filter_conditions() {
        // Both edge-filter forms get full expression checking with the
        // edge table as the row.
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD since ON likes TYPE datetime;\nSELECT * FROM person->likes[WHERE since > 5]->post;\nSELECT * FROM person->(likes WHERE since + 1)->post;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .map(|finding| (finding.code().to_string(), finding.message().to_string()))
            .collect();

        // One operand contract, one code — SurrealDB tolerating the
        // comparison (it kind-orders) changes nothing.
        assert!(messages.contains(&(
            "E2004".to_string(),
            "`>` can't combine a `datetime` and a `int`".to_string()
        )));
        assert!(messages.contains(&(
            "E2004".to_string(),
            "`+` can't combine a `datetime` and a `int`".to_string()
        )));
    }

    #[test]
    fn analyze_workspace_reports_unreachable_graph_hop_targets() {
        // `likes` goes to `post`; landing on `comment` is 3003.
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE comment;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT * FROM person->likes->comment;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output.sources[&source]
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::graph(3002))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec!["relation `likes` connects `person`->`likes`->`post`, so this hop cannot land on `comment`"]
        );
    }

    #[test]
    fn analyze_workspace_reports_mismatched_graph_relation_endpoints() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT * FROM post->likes->person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::graph(3002))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec!["relation `likes` connects `person`->`likes`->`post`, but this step traverses `->` from `post`"]
        );
    }

    #[test]
    fn analyze_workspace_allows_matching_relate_relation_endpoints() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nRELATE person:one->likes->post:one;".into(),
        );

        let output = analyze_workspace(&workspace);

        assert!(output
            .diagnostics
            .iter()
            .all(|finding| finding.code() != FindingCode::graph(3002)));
    }

    #[test]
    fn analyze_workspace_reports_mismatched_relate_relation_endpoints() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nRELATE post:one->likes->person:one;".into(),
        );

        let output = analyze_workspace(&workspace);
        let mismatches: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::graph(3002))
            .collect();

        let messages: Vec<_> = mismatches
            .iter()
            .map(|finding| finding.message().to_string())
            .collect();
        // One finding comparing the declared shape against the written one.
        assert_eq!(
            messages,
            vec![
                "relation `likes` connects `person`->`likes`->`post`, but this RELATE writes `post`->`likes`->`person`",
            ]
        );
        assert_eq!(mismatches[0].span().source(), &source);
        assert_eq!(output.sources[&source].diagnostics.len(), 1);
    }

    #[test]
    fn analyze_workspace_reports_unknown_relate_edge_and_target_tables() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nRELATE person:one->missing->post:one;\nRELATE person:one->likes->ghost:one;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| matches!(finding.code(), code if code == FindingCode::schema(1001) || code == FindingCode::from_number(1001)))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec![
                "`missing` is not a defined table",
                "`ghost` is not a defined table",
                "`likes` is not a defined table",
            ]
        );
    }

    #[test]
    fn analyze_workspace_validates_parenthesized_graph_where_against_edge_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD created_at ON likes TYPE datetime;\nSELECT * FROM person->(likes WHERE created_at > $since)->post;\nSELECT * FROM person->(likes WHERE missing_since > $since)->post;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1002))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec!["`likes` has no field `missing_since`"]
        );
    }

    #[test]
    fn analyze_workspace_validates_bracketed_graph_filter_against_edge_fields() {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD created_at ON likes TYPE datetime;\nSELECT * FROM person->likes[WHERE created_at > $since]->post;\nSELECT * FROM person->likes[WHERE missing_since > $since]->post;".into(),
        );

        let output = analyze_workspace(&workspace);
        let messages: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1002))
            .map(|finding| finding.message().to_string())
            .collect();

        assert_eq!(
            messages,
            vec!["`likes` has no field `missing_since`"]
        );
    }

    /// A parsed fixture must reach semantic analysis: any `S`-category
    /// finding means a syntax error short-circuited the pipeline, so a
    /// semantic assertion below would be vacuous.
    fn assert_no_syntax_findings(diagnostics: &[Finding]) {
        assert!(
            !diagnostics
                .iter()
                .any(|finding| finding.code().to_string().starts_with('S')),
            "fixture failed to parse: {:?}",
            diagnostics
                .iter()
                .map(|f| (f.code().to_string(), f.message().to_string()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn analyze_workspace_reports_index_on_non_collection() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD age ON person TYPE int;\nSELECT age[0] FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let diagnostics = &output.sources[&source].diagnostics;
        assert_no_syntax_findings(diagnostics);

        let messages: Vec<_> = diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::type_error(2030))
            .map(|finding| finding.message().to_string())
            .collect();
        assert_eq!(
            messages,
            vec!["a `int` can't be indexed or filtered — it is not a collection"]
        );
    }

    #[test]
    fn analyze_workspace_reports_graph_step_through_non_relation_table() {
        let mut workspace = Workspace::default();
        // A multi-target step `->(likes, post)` requires every named edge to
        // be a relation table; `post` is a plain table, so it trips 3001.
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT ->(likes, post) FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let diagnostics = &output.sources[&source].diagnostics;
        assert_no_syntax_findings(diagnostics);

        let messages: Vec<_> = diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::graph(3001))
            .map(|finding| finding.message().to_string())
            .collect();
        assert_eq!(messages, vec!["`post` can't be traversed — it is not a relation table"]);
    }

    #[test]
    fn suppression_directive_silences_next_line_and_trailing_findings() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "-- surrealguard: allow(E1001) reason=\"fixture table\"\nSELECT * FROM ghost;\nSELECT * FROM phantom; -- surrealguard: allow(E1001) reason=\"also fine\"\nSELECT * FROM spectre;".into(),
        );

        let output = analyze_workspace(&workspace);
        let diagnostics = &output.sources[&source].diagnostics;
        assert_no_syntax_findings(diagnostics);

        // ghost (next-line) and phantom (trailing) are suppressed;
        // spectre still fires.
        let unknown_tables: Vec<_> = diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::schema(1001))
            .map(|finding| finding.message().to_string())
            .collect();
        assert_eq!(unknown_tables, vec!["`spectre` is not a defined table"]);
    }

    #[test]
    fn suppression_directive_contract_violations_are_7013() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "-- surrealguard: allow(E9999)\n-- surrealguard: allow(lint.select_star)\n-- surrealguard: allow(*)\nSELECT * FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let diagnostics = &output.sources[&source].diagnostics;

        let directive_findings: Vec<_> = diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::lint(7013))
            .map(|finding| finding.message().to_string())
            .collect();
        assert_eq!(directive_findings.len(), 3, "{directive_findings:?}");
        assert!(directive_findings[0].contains("`E9999` is not a catalog code"));
        assert!(directive_findings[1].contains("suppress by catalog code, not name"));
        assert!(directive_findings[2].contains("does not parse"));
    }

    #[test]
    fn suppression_reasons_are_required_when_configured() {
        let mut config = WorkspaceConfig::default();
        config.diagnostics.require_suppression_reasons = true;
        let mut workspace = Workspace::new(config);
        let source = workspace.add_virtual_source(
            "query".into(),
            "-- surrealguard: allow(E1001)\nSELECT * FROM ghost;".into(),
        );

        let output = analyze_workspace(&workspace);
        let diagnostics = &output.sources[&source].diagnostics;

        // Without a reason the directive is rejected: 7013 fires and the
        // 1001 it tried to silence still reports.
        assert!(diagnostics
            .iter()
            .any(|finding| finding.code() == FindingCode::lint(7013)));
        assert!(diagnostics
            .iter()
            .any(|finding| finding.code() == FindingCode::schema(1001)));
    }

    #[test]
    fn analyze_workspace_reports_casts_to_unknown_types() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "RETURN <ghost> 5;\nRETURN <int> '12';".into(),
        );

        let output = analyze_workspace(&workspace);
        let diagnostics = &output.sources[&source].diagnostics;
        assert_no_syntax_findings(diagnostics);

        let messages: Vec<_> = diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::type_error(2007))
            .map(|finding| finding.message().to_string())
            .collect();
        assert_eq!(messages, vec!["`ghost` is not a known type"]);
    }

    #[test]
    fn analyze_workspace_reports_single_step_traversal_through_plain_table() {
        let mut workspace = Workspace::default();
        // With no edge to land from, a plain table in step position is a
        // traversal, and a traversal must name a relation. The valid hop
        // form `->likes->post` stays quiet.
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nSELECT ->post FROM person;\nSELECT ->likes->post FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let diagnostics = &output.sources[&source].diagnostics;
        assert_no_syntax_findings(diagnostics);

        let messages: Vec<_> = diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::graph(3001))
            .map(|finding| finding.message().to_string())
            .collect();
        assert_eq!(messages, vec!["`post` can't be traversed — it is not a relation table"]);
    }

    #[test]
    fn analyze_workspace_reports_schemaless_table_when_typed_tables_exist() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE TABLE bare;\nSELECT * FROM bare;".into(),
        );

        let output = analyze_workspace(&workspace);
        let diagnostics = &output.sources[&source].diagnostics;
        assert_no_syntax_findings(diagnostics);

        let messages: Vec<_> = diagnostics
            .iter()
            .filter(|finding| finding.code() == FindingCode::lint(7008))
            .map(|finding| finding.message().to_string())
            .collect();
        assert_eq!(
            messages,
            vec!["`bare` has no declared fields, so field-level checks are skipped"]
        );
    }

    #[test]
    fn analyze_workspace_reports_possibly_none_operand_in_arithmetic() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD nick ON person TYPE option<string>;\nSELECT nick + 'x' FROM person;".into(),
        );

        let output = analyze_workspace(&workspace);
        let diagnostics = &output.sources[&source].diagnostics;
        assert_no_syntax_findings(diagnostics);

        assert!(
            diagnostics.iter().any(|finding| {
                finding.code() == FindingCode::type_error(2015)
                    && finding.message().contains("may be NONE here")
            }),
            "expected 2015 for the option<string> operand, have: {:?}",
            diagnostics
                .iter()
                .map(|f| (f.code().to_string(), f.message().to_string()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn analyze_workspace_exposes_group_by_collapsing_select_modifier_facts() {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source(
            "query".into(),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nSELECT name FROM person GROUP BY name;".into(),
        );

        let output = analyze_workspace(&workspace);
        assert_no_syntax_findings(&output.sources[&source].diagnostics);
        let select = output.sources[&source]
            .statements
            .iter()
            .find(|statement| statement.kind == "select")
            .expect("select statement exists");

        // GROUP BY aggregates rows, so the fact is not row-preserving.
        let group = select
            .select_modifiers
            .iter()
            .find(|modifier| modifier.kind == "group")
            .expect("group modifier fact exists");
        assert!(!group.row_preserving);
    }

    #[test]
    fn analyze_workspace_accepts_a_diamond_function_call_graph() {
        let mut workspace = Workspace::default();
        // fn::a fans out to b and c, both call d, d calls nothing. A DAG,
        // not a cycle, so no non-termination (5009) finding.
        let source = workspace.add_virtual_source(
            "query".into(),
            concat!(
                "DEFINE FUNCTION fn::a() { RETURN fn::b() + fn::c(); };\n",
                "DEFINE FUNCTION fn::b() { RETURN fn::d(); };\n",
                "DEFINE FUNCTION fn::c() { RETURN fn::d(); };\n",
                "DEFINE FUNCTION fn::d() { RETURN 1; };\n",
            )
            .into(),
        );

        let output = analyze_workspace(&workspace);
        assert_no_syntax_findings(&output.sources[&source].diagnostics);
        assert!(
            !output
                .diagnostics
                .iter()
                .any(|finding| finding.code().to_string().ends_with("5009")),
            "diamond call graph must not report non-termination, have: {:?}",
            output
                .diagnostics
                .iter()
                .map(|f| (f.code().to_string(), f.message().to_string()))
                .collect::<Vec<_>>()
        );
    }

    // ---- Incremental single-source analysis equivalence ----
    //
    // The whole risk of the incremental path is a stale/wrong result. These
    // tests prove that, for a source that contributes nothing to the global
    // catalog (a pure query source), re-analyzing it in isolation against a
    // prebuilt `GlobalCatalog` is byte-identical to what the full
    // `analyze_workspace` pass produces for that source.

    /// Parses every registered source in a workspace, in registration order,
    /// so the parsed set feeds `build_global_catalog` with the same source ids
    /// the full pass uses.
    fn parsed_sources_of(workspace: &Workspace) -> Vec<surrealguard_syntax::parse::ParsedSource> {
        workspace
            .registry()
            .source_ids()
            .filter_map(|id| {
                let text = workspace.registry().text(id)?;
                parse_source(id.clone(), text).ok()
            })
            .collect()
    }

    /// Asserts the incremental output for `target` equals the full-pass output
    /// for `target` on every dimension the task pins.
    fn assert_incremental_matches_full(
        schema_sources: &[&str],
        query_text: &str,
    ) {
        let mut workspace = Workspace::default();
        for (i, schema) in schema_sources.iter().enumerate() {
            workspace.add_virtual_source(format!("schema{i}"), (*schema).into());
        }
        let target = workspace.add_virtual_source("query".into(), query_text.into());

        let full = analyze_workspace(&workspace);
        let full_target = full
            .sources
            .get(&target)
            .expect("target analyzed in full pass")
            .clone();

        // Build the catalog from ONLY the schema sources — the incremental path
        // never re-parses the query into the catalog. This mirrors caching the
        // catalog across a query-file edit.
        let parsed = parsed_sources_of(&workspace);
        let catalog = build_global_catalog(&parsed);
        let parsed_target = parsed
            .iter()
            .find(|p| p.source_id() == &target)
            .expect("target parsed");

        let incremental = analyze_one_source(&catalog, parsed_target, false);

        assert_eq!(
            incremental.diagnostics, full_target.diagnostics,
            "diagnostics diverged for query `{query_text}`"
        );
        assert_eq!(
            incremental.response_kind, full_target.response_kind,
            "response_kind diverged for query `{query_text}`"
        );
        assert_eq!(
            incremental.let_bindings, full_target.let_bindings,
            "let_bindings diverged for query `{query_text}`"
        );
        assert_eq!(
            incremental.statements, full_target.statements,
            "statements diverged for query `{query_text}`"
        );
        assert_eq!(
            incremental.inferred_params, full_target.inferred_params,
            "inferred_params diverged for query `{query_text}`"
        );
    }

    #[test]
    fn incremental_query_analysis_matches_full_across_a_battery() {
        let schema = &[
            "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             DEFINE FIELD age ON person TYPE int;\n\
             DEFINE FIELD email ON person TYPE option<string>;",
            "DEFINE TABLE post SCHEMAFULL;\n\
             DEFINE FIELD title ON post TYPE string;\n\
             DEFINE FIELD author ON post TYPE record<person>;\n\
             DEFINE FUNCTION fn::adult($p: record<person>) { RETURN (SELECT VALUE age FROM ONLY $p) >= 18; };",
        ];

        // A representative battery of pure-query sources: clean, diagnostic-
        // producing, param-bearing, LET-binding, cross-source-UDF, and
        // unknown-table.
        for query in [
            "SELECT name, age FROM person;",
            "SELECT * FROM person WHERE age > $min;",
            "LET $p = person:one;\nSELECT name FROM $p;",
            "SELECT title, author.name FROM post;",
            "RETURN fn::adult(person:one);",
            "SELECT * FROM ghost;",
            "SELECT name FROM person WHERE email != NONE;",
            "UPDATE person SET name = 'Ada' WHERE age < $max RETURN name;",
            "SELECT math::sum(age) AS total FROM person GROUP BY name;",
            "LET $unused = 1;\nRETURN 5;",
        ] {
            assert_incremental_matches_full(schema, query);
        }
    }

    #[test]
    fn incremental_query_analysis_matches_full_with_no_schema() {
        // No schema sources at all: the catalog is empty, and unknown-table and
        // opt-in lints must still match exactly.
        for query in [
            "SELECT * FROM person;",
            "RETURN 1 + 2;",
            "LET $x = 'a';\nRETURN $x;",
        ] {
            assert_incremental_matches_full(&[], query);
        }
    }

    /// Measurement hook (ignored by default): times a full whole-workspace
    /// pass against a single-source incremental re-analysis on a ~20-file
    /// corpus. Run with:
    ///   cargo test -p surrealguard-workspace incremental_reanalysis_speedup -- --ignored --nocapture
    #[test]
    #[ignore]
    fn incremental_reanalysis_speedup() {
        use std::time::Instant;

        let mut workspace = Workspace::default();
        // Four schema files.
        for f in 0..4 {
            let mut schema = String::new();
            for t in 0..5 {
                schema.push_str(&format!(
                    "DEFINE TABLE t{f}_{t} SCHEMAFULL;\n\
                     DEFINE FIELD name ON t{f}_{t} TYPE string;\n\
                     DEFINE FIELD n ON t{f}_{t} TYPE int;\n\
                     DEFINE FIELD tags ON t{f}_{t} TYPE array<string>;\n\
                     DEFINE FUNCTION fn::f{f}_{t}($x: record<t{f}_{t}>) {{ RETURN (SELECT VALUE n FROM ONLY $x); }};\n"
                ));
            }
            workspace.add_virtual_source(format!("schema{f}"), schema);
        }
        // Sixteen query files; keep a handle on one to re-analyze.
        let mut target = None;
        for q in 0..16 {
            let id = workspace.add_virtual_source(
                format!("query{q}"),
                format!(
                    "SELECT name, n, tags FROM t0_0 WHERE n > $min;\n\
                     RETURN fn::f1_1(t1_1:one);\n\
                     LET $rows = SELECT name FROM t2_2;\n\
                     UPDATE t3_3 SET name = 'x' WHERE n < $max RETURN name;"
                ),
            );
            if q == 8 {
                target = Some(id);
            }
        }
        let target = target.unwrap();

        let iters = 200;

        // Full whole-workspace pass per "keystroke".
        let full_start = Instant::now();
        for _ in 0..iters {
            let _ = analyze_workspace(&workspace);
        }
        let full = full_start.elapsed() / iters;

        // Incremental: catalog cached, only the dirty source re-analyzed.
        let parsed = parsed_sources_of(&workspace);
        let catalog = build_global_catalog(&parsed);
        let parsed_target = parsed
            .iter()
            .find(|p| p.source_id() == &target)
            .expect("target parsed");
        let incr_start = Instant::now();
        for _ in 0..iters {
            let _ = analyze_one_source(&catalog, parsed_target, false);
        }
        let incr = incr_start.elapsed() / iters;

        // Catalog build cost (paid once per schema change, amortized across
        // every subsequent query edit).
        let cat_start = Instant::now();
        for _ in 0..iters {
            let _ = build_global_catalog(&parsed);
        }
        let cat = cat_start.elapsed() / iters;

        eprintln!(
            "20-file corpus: full={full:?}  incremental={incr:?}  catalog_build={cat:?}  speedup={:.1}x",
            full.as_secs_f64() / incr.as_secs_f64().max(f64::MIN_POSITIVE)
        );
    }

    #[test]
    fn incremental_catalog_is_reused_across_successive_query_edits() {
        // Prove the catalog built once from the schema is reusable across a
        // sequence of query-file edits: each edit's incremental output matches
        // a fresh full pass with that query. This is exactly the LSP fast path.
        let schema = "DEFINE TABLE person SCHEMAFULL;\n\
                      DEFINE FIELD name ON person TYPE string;\n\
                      DEFINE FIELD age ON person TYPE int;";

        // Build the catalog once (schema only).
        let mut schema_ws = Workspace::default();
        schema_ws.add_virtual_source("schema".into(), schema.into());
        let schema_parsed = parsed_sources_of(&schema_ws);
        let catalog = build_global_catalog(&schema_parsed);

        for query in [
            "SELECT name FROM person;",
            "SELECT age FROM person WHERE age > 18;",
            "SELECT * FROM person;",
            "SELECT missing FROM person;",
        ] {
            // Full reference: a workspace whose schema source is registered
            // first (same id ordering as the catalog build above), then query.
            let mut full_ws = Workspace::default();
            full_ws.add_virtual_source("schema".into(), schema.into());
            let target = full_ws.add_virtual_source("query".into(), query.into());
            let full = analyze_workspace(&full_ws);
            let full_target = full.sources.get(&target).expect("target").clone();

            // Incremental: parse only the query, reusing the cached catalog. The
            // query's source id must match `target` for spans to line up — which
            // they do because both workspaces register schema-then-query.
            let parsed_query = parse_source(target.clone(), query).expect("query parses");
            let incremental = analyze_one_source(&catalog, &parsed_query, false);

            assert_eq!(incremental.diagnostics, full_target.diagnostics, "query `{query}`");
            assert_eq!(incremental.response_kind, full_target.response_kind, "query `{query}`");
            assert_eq!(incremental.let_bindings, full_target.let_bindings, "query `{query}`");
        }
    }
}

#[cfg(test)]
mod symbol_incremental_tests {
    //! Equivalence + granularity coverage for symbol-level incremental
    //! re-analysis. The invariant under test: merging the previous per-source
    //! outputs with a fresh re-analysis of only the AFFECTED sources must equal
    //! a from-scratch `analyze_workspace` at the post-edit state, for EVERY
    //! source. A stale (unaffected-but-should-have-changed) source fails the
    //! merge check, which is exactly the correctness bug we must never ship.

    use super::*;
    use std::path::PathBuf;

    fn sid(name: &str) -> SourceId {
        SourceId::new(format!("file:///{name}"))
    }

    fn parsed_for(sources: &[(&str, &str)]) -> Vec<ParsedSource> {
        sources
            .iter()
            .map(|(name, text)| parse_source(sid(name), *text).expect("test sources parse"))
            .collect()
    }

    /// Ground truth: a from-scratch whole-workspace pass over `sources`, using
    /// file ids so they align with the incremental path.
    fn full_workspace(sources: &[(&str, &str)]) -> WorkspaceAnalysis {
        let mut workspace = Workspace::default();
        for (name, text) in sources {
            workspace.add_file_source(PathBuf::from(format!("/{name}")), (*text).to_string());
        }
        analyze_workspace(&workspace)
    }

    /// Computes the affected set for an edit exactly as the LSP does, then
    /// returns `(affected_ids, merged_outputs)` where merged = the before-state
    /// outputs with the affected sources replaced by a fresh re-analysis.
    fn incremental(
        before: &[(&str, &str)],
        after: &[(&str, &str)],
    ) -> (BTreeSet<SourceId>, BTreeMap<SourceId, AnalysisOutput>) {
        assert_eq!(before.len(), after.len(), "harness expects a same-doc-set edit");
        let before_parsed = parsed_for(before);
        let after_parsed = parsed_for(after);
        let before_catalog = build_global_catalog(&before_parsed);
        let after_catalog = build_global_catalog(&after_parsed);

        let dirty: BTreeSet<SourceId> = before
            .iter()
            .zip(after)
            .filter(|((_, bt), (_, at))| bt != at)
            .map(|((name, _), _)| sid(name))
            .collect();
        // The harness only covers edits the symbol path actually takes; an
        // unmodeled dirty effect would force the (trivially equivalent) full
        // pass in the LSP.
        for parsed in &after_parsed {
            if dirty.contains(parsed.source_id()) {
                assert!(
                    !source_requires_full_reanalysis(parsed),
                    "dirty source {} carries an unmodeled effect; test the full-fallback instead",
                    parsed.source_id()
                );
            }
        }

        let changed = changed_symbols(&before_catalog, &after_catalog);
        let mut affected = dirty.clone();
        for parsed in &after_parsed {
            if source_reference_set(parsed).is_affected_by(&changed) {
                affected.insert(parsed.source_id().clone());
            }
        }
        for id in sources_with_changed_cycle_findings(&before_catalog, &after_catalog) {
            affected.insert(id);
        }

        let reanalyzed = reanalyze_sources(&after_parsed, &after_catalog, &affected, false);
        let mut merged: BTreeMap<SourceId, AnalysisOutput> =
            full_workspace(before).sources;
        for (id, output) in &reanalyzed {
            merged.insert(id.clone(), output.clone());
        }
        (affected, merged)
    }

    /// Asserts the merged incremental result equals a full pass at `after` for
    /// every source (diagnostics, response kind, statements, params, let
    /// bindings) plus the rebuilt schema. Returns the affected-source count so
    /// callers can also assert granularity.
    fn assert_equivalent(before: &[(&str, &str)], after: &[(&str, &str)]) -> usize {
        let (affected, merged) = incremental(before, after);
        let after_full = full_workspace(after);

        for (id, expected) in &after_full.sources {
            let got = merged
                .get(id)
                .unwrap_or_else(|| panic!("merged result missing source {id}"));
            assert_eq!(got.diagnostics, expected.diagnostics, "diagnostics for {id}");
            assert_eq!(
                got.response_kind, expected.response_kind,
                "response_kind for {id}"
            );
            assert_eq!(got.statements, expected.statements, "statements for {id}");
            assert_eq!(
                got.inferred_params, expected.inferred_params,
                "inferred_params for {id}"
            );
            assert_eq!(got.let_bindings, expected.let_bindings, "let_bindings for {id}");
        }

        // The incrementally-rebuilt schema must match a full pass' schema so
        // editor features resolve against the current catalog.
        let after_parsed = parsed_for(after);
        assert_eq!(
            build_workspace_schema(&after_parsed),
            after_full.schema,
            "rebuilt schema must equal the full-pass schema"
        );

        affected.len()
    }

    const SCHEMA: &str = "DEFINE TABLE person SCHEMAFULL;\n\
         DEFINE FIELD name ON person TYPE string;\n\
         DEFINE FIELD age ON person TYPE int;";

    // (a) A query-file edit.
    #[test]
    fn query_edit_is_equivalent() {
        let before = &[
            ("schema.surql", SCHEMA),
            ("q.surql", "SELECT name FROM person;"),
        ];
        let after = &[
            ("schema.surql", SCHEMA),
            ("q.surql", "SELECT name, age FROM person WHERE age > 18;"),
        ];
        assert_equivalent(before, after);
    }

    // (b) A function-body edit that CHANGES its inferred return, read by a caller.
    #[test]
    fn function_body_edit_changing_return_reanalyzes_callers() {
        let before = &[
            (
                "fns.surql",
                "DEFINE FUNCTION fn::pick() { RETURN 1; };",
            ),
            ("caller.surql", "RETURN fn::pick();"),
        ];
        let after = &[
            (
                "fns.surql",
                "DEFINE FUNCTION fn::pick() { RETURN 'text'; };",
            ),
            ("caller.surql", "RETURN fn::pick();"),
        ];
        let (affected, _) = incremental(before, after);
        assert!(
            affected.contains(&sid("caller.surql")),
            "the caller must re-analyze when the callee's return type changes"
        );
        assert_equivalent(before, after);
    }

    // (c) A function-body edit that does NOT change its signature.
    #[test]
    fn function_body_edit_preserving_signature_is_equivalent() {
        let before = &[
            (
                "fns.surql",
                "DEFINE FUNCTION fn::pick() -> int { RETURN 1; };",
            ),
            ("caller.surql", "RETURN fn::pick();"),
        ];
        let after = &[
            (
                "fns.surql",
                "DEFINE FUNCTION fn::pick() -> int { RETURN 1 + 1; };",
            ),
            ("caller.surql", "RETURN fn::pick();"),
        ];
        assert_equivalent(before, after);
    }

    // (d) A field TYPE change.
    #[test]
    fn field_type_change_is_equivalent() {
        let before = &[
            (
                "schema.surql",
                "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD age ON person TYPE int;",
            ),
            ("q.surql", "SELECT age FROM person;"),
        ];
        let after = &[
            (
                "schema.surql",
                "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD age ON person TYPE string;",
            ),
            ("q.surql", "SELECT age FROM person;"),
        ];
        let (affected, _) = incremental(before, after);
        assert!(
            affected.contains(&sid("q.surql")),
            "a reader of the changed field must re-analyze"
        );
        assert_equivalent(before, after);
    }

    // (e) Editing a function OTHER sources call: every caller re-analyzes.
    #[test]
    fn editing_a_called_function_reanalyzes_every_caller() {
        let before = &[
            (
                "fns.surql",
                "DEFINE FUNCTION fn::v() { RETURN 1; };",
            ),
            ("a.surql", "RETURN fn::v();"),
            ("b.surql", "RETURN fn::v() + 1;"),
            ("c.surql", "RETURN 42;"),
        ];
        let after = &[
            (
                "fns.surql",
                "DEFINE FUNCTION fn::v() { RETURN 'x'; };",
            ),
            ("a.surql", "RETURN fn::v();"),
            ("b.surql", "RETURN fn::v() + 1;"),
            ("c.surql", "RETURN 42;"),
        ];
        let (affected, _) = incremental(before, after);
        assert!(affected.contains(&sid("a.surql")));
        assert!(affected.contains(&sid("b.surql")));
        assert!(
            !affected.contains(&sid("c.surql")),
            "a source that never calls the function must NOT re-analyze"
        );
        assert_equivalent(before, after);
    }

    // (f) A definition removed from a file (no REMOVE statement — the DEFINE
    // text is deleted): the catalog loses the table, and its readers must see
    // the new unknown-table finding.
    #[test]
    fn removing_a_definition_reanalyzes_readers() {
        let before = &[
            ("schema.surql", "DEFINE TABLE ghost;\nDEFINE TABLE person;"),
            ("q.surql", "SELECT * FROM ghost;"),
        ];
        let after = &[
            ("schema.surql", "DEFINE TABLE person;"),
            ("q.surql", "SELECT * FROM ghost;"),
        ];
        let (affected, _) = incremental(before, after);
        assert!(affected.contains(&sid("q.surql")));
        assert_equivalent(before, after);
    }

    // (f') Adding a definition a reader depends on.
    #[test]
    fn adding_a_definition_reanalyzes_readers() {
        let before = &[
            ("schema.surql", "DEFINE TABLE person;"),
            ("q.surql", "SELECT * FROM ghost;"),
        ];
        let after = &[
            ("schema.surql", "DEFINE TABLE person;\nDEFINE TABLE ghost;"),
            ("q.surql", "SELECT * FROM ghost;"),
        ];
        assert_equivalent(before, after);
    }

    // A REMOVE statement in a dirty doc forces the full fallback (unmodeled).
    #[test]
    fn remove_statement_requires_full_reanalysis() {
        let parsed = parse_source(sid("s.surql"), "REMOVE TABLE person;").expect("parse");
        assert!(source_requires_full_reanalysis(&parsed));
        let alter = parse_source(sid("s.surql"), "ALTER TABLE person DROP;").expect("parse");
        assert!(source_requires_full_reanalysis(&alter));
        let define_param =
            parse_source(sid("s.surql"), "DEFINE PARAM $x VALUE 1;").expect("parse");
        assert!(source_requires_full_reanalysis(&define_param));
        let plain = parse_source(sid("s.surql"), "DEFINE TABLE person;").expect("parse");
        assert!(!source_requires_full_reanalysis(&plain));
    }

    // (g) An edit to a source many others reference: all readers re-analyze and
    // the result stays equivalent.
    #[test]
    fn editing_a_widely_referenced_table_reanalyzes_all_readers() {
        let before = &[
            (
                "schema.surql",
                "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;",
            ),
            ("q1.surql", "SELECT name FROM person;"),
            ("q2.surql", "SELECT name FROM person WHERE name != NONE;"),
            ("q3.surql", "CREATE person SET name = 'a';"),
            ("unrelated.surql", "RETURN 1 + 1;"),
        ];
        let after = &[
            (
                "schema.surql",
                "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE int;",
            ),
            ("q1.surql", "SELECT name FROM person;"),
            ("q2.surql", "SELECT name FROM person WHERE name != NONE;"),
            ("q3.surql", "CREATE person SET name = 'a';"),
            ("unrelated.surql", "RETURN 1 + 1;"),
        ];
        let (affected, _) = incremental(before, after);
        assert!(!affected.contains(&sid("unrelated.surql")));
        assert_equivalent(before, after);
    }

    // Mutual recursion: editing one function to close a cycle must flip the
    // 5009 findings of BOTH functions' sources, even though only one is dirty.
    #[test]
    fn closing_a_recursion_cycle_reanalyzes_the_other_function() {
        let before = &[
            ("a.surql", "DEFINE FUNCTION fn::a() { RETURN fn::b(); };"),
            ("b.surql", "DEFINE FUNCTION fn::b() { RETURN 1; };"),
        ];
        let after = &[
            ("a.surql", "DEFINE FUNCTION fn::a() { RETURN fn::b(); };"),
            ("b.surql", "DEFINE FUNCTION fn::b() { RETURN fn::a(); };"),
        ];
        let (affected, _) = incremental(before, after);
        assert!(
            affected.contains(&sid("a.surql")),
            "the other function in the newly-formed cycle must re-analyze for 5009"
        );
        assert_equivalent(before, after);
    }

    /// Measurement hook (ignored by default): on a ~200-file synthetic corpus,
    /// times a full whole-workspace pass against the symbol-incremental schema
    /// path for a function-body edit, and reports how many sources re-analyze.
    /// Run with:
    ///   cargo test -p surrealguard-workspace symbol_incremental_schema_speedup -- --ignored --nocapture
    #[test]
    #[ignore]
    fn symbol_incremental_schema_speedup() {
        use std::time::Instant;

        // 20 schema files (100 tables, 20 helper functions) + 180 query files.
        let mut before: Vec<(String, String)> = Vec::new();
        for f in 0..20 {
            let mut schema = String::new();
            for t in 0..5 {
                schema.push_str(&format!(
                    "DEFINE TABLE t{f}_{t} SCHEMAFULL;\n\
                     DEFINE FIELD name ON t{f}_{t} TYPE string;\n\
                     DEFINE FIELD n ON t{f}_{t} TYPE int;\n\
                     DEFINE FUNCTION fn::f{f}_{t}($x: record<t{f}_{t}>) {{ RETURN (SELECT VALUE n FROM ONLY $x); }};\n"
                ));
            }
            before.push((format!("schema{f}.surql"), schema));
        }
        for q in 0..180 {
            let f = q % 20;
            let t = q % 5;
            before.push((
                format!("query{q}.surql"),
                format!(
                    "SELECT name, n FROM t{f}_{t} WHERE n > $min;\n\
                     RETURN fn::f{f}_{t}(t{f}_{t}:one);"
                ),
            ));
        }
        // Edit ONE helper's body (add an arithmetic op). Nothing outside its own
        // file changes shape, but its callers read its return, so they re-run.
        let mut after = before.clone();
        after[0].1 = after[0].1.replacen(
            "DEFINE FUNCTION fn::f0_0($x: record<t0_0>) { RETURN (SELECT VALUE n FROM ONLY $x); };",
            "DEFINE FUNCTION fn::f0_0($x: record<t0_0>) { RETURN (SELECT VALUE n FROM ONLY $x) + 1; };",
            1,
        );

        let before_refs: Vec<(&str, &str)> =
            before.iter().map(|(n, t)| (n.as_str(), t.as_str())).collect();
        let after_refs: Vec<(&str, &str)> =
            after.iter().map(|(n, t)| (n.as_str(), t.as_str())).collect();

        let iters = 20;
        // Full whole-workspace pass per keystroke.
        let full_start = Instant::now();
        for _ in 0..iters {
            let _ = full_workspace(&after_refs);
        }
        let full = full_start.elapsed() / iters;

        // Symbol-incremental path per keystroke: rebuild the catalog (O(defines)),
        // diff it, and re-analyze only the affected sources.
        let before_parsed = parsed_for(&before_refs);
        let before_catalog = build_global_catalog(&before_parsed);
        let mut affected_count = 0usize;
        let incr_start = Instant::now();
        for _ in 0..iters {
            let after_parsed = parsed_for(&after_refs);
            let after_catalog = build_global_catalog(&after_parsed);
            let changed = changed_symbols(&before_catalog, &after_catalog);
            let mut affected: BTreeSet<SourceId> = [sid("schema0.surql")].into_iter().collect();
            for parsed in &after_parsed {
                if source_reference_set(parsed).is_affected_by(&changed) {
                    affected.insert(parsed.source_id().clone());
                }
            }
            for id in sources_with_changed_cycle_findings(&before_catalog, &after_catalog) {
                affected.insert(id);
            }
            affected_count = affected.len();
            let _ = reanalyze_sources(&after_parsed, &after_catalog, &affected, false);
        }
        let incr = incr_start.elapsed() / iters;

        eprintln!(
            "200-file corpus [body edit, return type unchanged]: sources={}  full={full:?}  symbol_incremental={incr:?}  reanalyzed={affected_count}  speedup={:.1}x",
            before.len(),
            full.as_secs_f64() / incr.as_secs_f64().max(f64::MIN_POSITIVE)
        );

        // Scenario B: the edit CHANGES the helper's return type (int -> string),
        // so every source that calls it re-analyzes — still far fewer than N.
        let mut after_b = before.clone();
        after_b[0].1 = after_b[0].1.replacen(
            "DEFINE FUNCTION fn::f0_0($x: record<t0_0>) { RETURN (SELECT VALUE n FROM ONLY $x); };",
            "DEFINE FUNCTION fn::f0_0($x: record<t0_0>) { RETURN 'label'; };",
            1,
        );
        let after_b_refs: Vec<(&str, &str)> =
            after_b.iter().map(|(n, t)| (n.as_str(), t.as_str())).collect();
        let mut affected_b = 0usize;
        let incr_b_start = Instant::now();
        for _ in 0..iters {
            let after_parsed = parsed_for(&after_b_refs);
            let after_catalog = build_global_catalog(&after_parsed);
            let changed = changed_symbols(&before_catalog, &after_catalog);
            let mut affected: BTreeSet<SourceId> = [sid("schema0.surql")].into_iter().collect();
            for parsed in &after_parsed {
                if source_reference_set(parsed).is_affected_by(&changed) {
                    affected.insert(parsed.source_id().clone());
                }
            }
            for id in sources_with_changed_cycle_findings(&before_catalog, &after_catalog) {
                affected.insert(id);
            }
            affected_b = affected.len();
            let _ = reanalyze_sources(&after_parsed, &after_catalog, &affected, false);
        }
        let incr_b = incr_b_start.elapsed() / iters;
        eprintln!(
            "200-file corpus [body edit, return type int->string]: full={full:?}  symbol_incremental={incr_b:?}  reanalyzed={affected_b}  speedup={:.1}x",
            full.as_secs_f64() / incr_b.as_secs_f64().max(f64::MIN_POSITIVE)
        );
    }

    // Granularity: a function-body edit whose function nothing references
    // re-analyzes ONLY the edited source, not the whole workspace.
    #[test]
    fn unreferenced_function_body_edit_reanalyzes_only_itself() {
        let mut before: Vec<(String, String)> = Vec::new();
        before.push((
            "helper.surql".to_string(),
            "DEFINE FUNCTION fn::helper() -> int { RETURN 1; };".to_string(),
        ));
        for i in 0..40 {
            before.push((
                format!("q{i}.surql"),
                format!("RETURN {i};"),
            ));
        }
        let before_refs: Vec<(&str, &str)> = before
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect();

        let mut after = before.clone();
        after[0].1 = "DEFINE FUNCTION fn::helper() -> int { RETURN 2; };".to_string();
        let after_refs: Vec<(&str, &str)> = after
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect();

        let affected = assert_equivalent(&before_refs, &after_refs);
        assert_eq!(
            affected, 1,
            "an unreferenced function-body edit re-analyzes exactly one source, not {}",
            before.len()
        );
    }
}
