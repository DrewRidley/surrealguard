//! The workspace analysis pipeline: one source-order walk per source.
//!
//! Each top-level statement is lowered and dispatched to its analyzer
//! (which emits both its response kind and its findings), against the
//! schema built from the statements before it. The walk also owns the
//! cross-statement contracts that no single statement can see: schema
//! effects, transaction pairing (4007), parameter constraints and their
//! source-order rules (6004), `fn::` termination (5009), and event
//! self-triggering (5010).
//!
//! The schema index is shared across the whole run and accumulates in
//! iteration order: a `DEFINE` in one source is visible to statements in
//! sources that come after it, never before. Parameter environments and
//! transaction state reset per source.
//!
//! Every stage consumes the lowered AST: statements arrive from
//! [`surrealql_analyzer_syntax::lower::lower_statements`], and schema effects apply
//! to those same lowered values.

use std::collections::{BTreeMap, BTreeSet};

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;
use surrealql_analyzer_syntax::parse::ParsedSource;
use surrealql_analyzer_syntax::source::SourceId;
use surrealql_analyzer_syntax::span::{ByteRange, SourceSpan};

use crate::analysis::{
    LetBindingAnalysis, NarrowingAnalysis, ParamInference, SelectModifierAnalysis,
    StatementAnalysis,
};
use crate::analyzer::context::AnalysisContext;
use crate::config::TargetVersion;
use crate::schema::{SchemaIndex, TableDef};
use crate::statement_env::StatementEnv;
use surrealql_analyzer_diagnostics::Finding;

#[derive(Debug, Default)]
pub(crate) struct PipelineOutput {
    pub schema: SchemaIndex,
    pub diagnostics: Vec<Finding>,
    pub sources: BTreeMap<SourceId, SourceAnalysis>,
}

#[derive(Debug, Default)]
pub(crate) struct SourceAnalysis {
    pub statements: Vec<StatementAnalysis>,
    pub params: Vec<ParamInference>,
    /// Every `LET`/`FOR` binding in the source, at every nesting depth.
    pub let_bindings: Vec<LetBindingAnalysis>,
    /// Every flow narrowing in the source, with the region it covers.
    pub narrowings: Vec<NarrowingAnalysis>,
}

/// The order-independent, cross-source outputs of the pipeline's pre-passes —
/// everything a single source's analysis reads about the *rest* of the
/// workspace. Building it is the expensive part of a whole-workspace pass that
/// does NOT depend on which source is being analyzed, so it can be built once
/// and reused to re-analyze one dirty source in isolation (see
/// [`analyze_one_source`]).
///
/// A source contributes to this catalog only through *additive* `DEFINE`s
/// (PRE-PASS 1/1b/1c), implicit schemaless-table targets (PRE-PASS 2), and its
/// `fn::` guardedness. A source with none of those (a pure query file) adds
/// nothing here, which is exactly what makes single-source re-analysis sound:
/// its `GlobalCatalog` is identical whether or not that source is present.
#[derive(Debug, Clone, Default)]
pub struct GlobalCatalog {
    /// PRE-PASS 1/1b/1c: the global additive `DEFINE` namespace, with function
    /// returns and untyped-field value kinds resolved against the full catalog.
    pub(crate) global_defined: SchemaIndex,
    /// PRE-PASS 1b: each untyped `fn::`'s globally-inferred return kind.
    pub(crate) global_fn_returns: BTreeMap<String, Kind>,
    /// PRE-PASS 1c: each untyped field's globally-inferred stored-value kind.
    pub(crate) global_field_kinds: BTreeMap<(String, String), Kind>,
    /// PRE-PASS 1d: each `DEFINE PARAM`'s default fact, keyed by param name
    /// (no `$`). A `DEFINE PARAM` is a database-side default: the value is
    /// supplied by the *database*, in every source, so it is a global fact
    /// exactly like a function's return kind.
    pub(crate) global_param_defaults: BTreeMap<String, crate::expression::ExpressionFact>,
    /// PRE-PASS 2: the implicit schemaless tables created on demand.
    pub(crate) implicit_tables: Vec<TableDef>,
    /// Whether each `fn::` body branches (guards 5009).
    pub(crate) fn_guarded: BTreeMap<String, bool>,
    /// The SurrealDB release the workspace targets
    /// (`analysis.surrealdb_version`), when configured. A workspace-wide fact
    /// like the rest of the catalog: every source is checked against the same
    /// engine, and the single-source re-analysis paths read it from here.
    pub(crate) target_version: Option<TargetVersion>,
}

impl GlobalCatalog {
    /// Records the configured target SurrealDB version; `None` (the default)
    /// is "the latest" and gates no 8xxx check.
    pub fn with_target_version(mut self, version: Option<TargetVersion>) -> Self {
        self.target_version = version;
        self
    }
}

/// One source paired with its lowered statements.
type LoweredSource<'a> = (&'a ParsedSource, Vec<ast::Spanned<ast::Statement>>);

/// Lowers every source once. Sources with syntax errors are kept — the lowerer
/// isolates each broken construct as `Statement::Partial` (inert in every pass),
/// so a syntax error in one statement never suppresses analysis of its
/// well-formed siblings.
///
/// Generic over `Borrow<ParsedSource>` so a caller can pass owned
/// `[ParsedSource]` (the full pass) or borrowed/`Arc`-shared handles (the
/// symbol-incremental LSP path, which reuses cached `ParsedSource`s for the
/// unchanged documents).
fn lower_all<P: std::borrow::Borrow<ParsedSource>>(parsed_sources: &[P]) -> Vec<LoweredSource<'_>> {
    parsed_sources
        .iter()
        .map(|parsed| {
            let parsed = parsed.borrow();
            (
                parsed,
                surrealql_analyzer_syntax::lower::lower_statements(parsed),
            )
        })
        .collect()
}

/// Runs PRE-PASS 1/1b/1c/2 (and `fn::` guardedness) over the whole source set,
/// producing the reusable [`GlobalCatalog`]. This is the single source of truth
/// for those pre-passes: both the full workspace walk and single-source
/// re-analysis consume its output, so they can never diverge.
pub fn build_global_catalog<P: std::borrow::Borrow<ParsedSource>>(
    parsed_sources: &[P],
) -> GlobalCatalog {
    let sources = lower_all(parsed_sources);
    build_global_catalog_from_lowered(&sources)
}

/// PRE-PASS 1 — the global additive catalog. A namespace is global: a
/// `DEFINE` in any source is a legitimate target for a reference in every
/// other source, regardless of file sort order. Only additive `DEFINE`
/// effects apply here; `REMOVE` and the order-sensitive contracts (4007,
/// 6004, duplicate definition) stay in the ordered walk. No inference runs,
/// so this is cheap enough for the schema-only rebuild path too.
fn build_global_defined(sources: &[LoweredSource<'_>]) -> SchemaIndex {
    let mut global_defined = SchemaIndex::default();
    for (parsed, statements) in sources {
        for stmt in statements {
            apply_additive_define(stmt, parsed.source_id(), parsed.text(), &mut global_defined);
        }
    }
    global_defined
}

fn build_global_catalog_from_lowered(sources: &[LoweredSource<'_>]) -> GlobalCatalog {
    let mut global_defined = build_global_defined(sources);

    // PRE-PASS 1b — global function return types. `apply_additive_define`
    // (above) built each function's body against an empty catalog, so a body
    // reading a *table* degraded to `Any`. A function's inferred return is
    // GLOBAL — independent of which source is being analyzed — so compute it
    // ONCE against the full catalog and reuse it in every source's working
    // catalog below. This is O(functions) inference, not O(sources × functions)
    // (an earlier per-source re-inference made large workspaces crawl). Results
    // are applied back to `global_defined` in source order so a function that
    // calls an earlier-defined untyped helper resolves through it.
    let mut global_fn_returns: BTreeMap<String, surrealdb_types::Kind> = BTreeMap::new();
    for (parsed, statements) in sources {
        for stmt in statements {
            let ast::Statement::Define(ast::DefineStmt::Function(def)) = &stmt.node else {
                continue;
            };
            match global_defined.function(&def.name.node) {
                Some(function)
                    if function.return_kind.is_none() && function.inferred_return.is_none() => {}
                _ => continue,
            }
            if let Some(kind) = crate::schema::infer_untyped_return(
                def,
                parsed.source_id(),
                parsed.text(),
                Some(&global_defined),
            ) {
                if let Some(function) = global_defined.functions.get_mut(&def.name.node) {
                    function.inferred_return = Some(kind.clone());
                }
                global_fn_returns.insert(def.name.node.clone(), kind);
            }
        }
    }

    // PRE-PASS 1c — global untyped-field value kinds. `field_def_from_ast`
    // (above, via `apply_additive_define`) built each untyped field's kind
    // against an empty catalog, so a `VALUE`/`COMPUTED` reading a *table* or
    // `fn::` helper degraded to `None`. An untyped field's stored-value kind is
    // GLOBAL — independent of which source is being analyzed — so compute it
    // ONCE against the full catalog and reuse it in every source's working
    // catalog below. This is O(fields) inference, not O(sources × fields).
    // Results are applied back to `global_defined` so a field whose value reads
    // an earlier-inferred field resolves through it.
    let mut global_field_kinds: BTreeMap<(String, String), Kind> = BTreeMap::new();
    for (parsed, statements) in sources {
        for stmt in statements {
            let ast::Statement::Define(ast::DefineStmt::Field(def)) = &stmt.node else {
                continue;
            };
            if def.ty.is_some() {
                continue;
            }
            let table = def.table.node.clone();
            let field_key = crate::schema::idiom_field_path(&def.path.node).join(".");
            match global_defined
                .tables
                .get(&table)
                .and_then(|t| t.fields.get(&field_key))
            {
                // Re-infer when the field has no kind yet, OR when PRE-PASS 1's
                // empty-catalog inference left an unresolved `Any` in it. A
                // `COMPUTED`/`VALUE` reading `$this.<sibling>` (or a table, or a
                // `fn::` helper) degrades to `Any` without the catalog, but the
                // `ELSE`/fall-through arm can still yield a concrete kind — so the
                // field lands as `Any | T`, non-`None`, and would otherwise be
                // skipped here. The full catalog is now built, so resolve it.
                Some(field)
                    if field
                        .kind
                        .as_ref()
                        .is_none_or(crate::schema::kind_contains_any) => {}
                _ => continue,
            }
            // No separate workspace catalog: `global_defined` already IS the
            // whole-workspace, order-independent one.
            if let Some(kind) = crate::schema::infer_field_value_kind(
                def,
                parsed.source_id(),
                parsed.text(),
                Some(&global_defined),
                None,
            ) {
                if let Some(field) = global_defined
                    .tables
                    .get_mut(&table)
                    .and_then(|t| t.fields.get_mut(&field_key))
                {
                    field.kind = Some(kind.clone());
                    field.partial.clear();
                }
                global_field_kinds.insert((table, field_key), kind);
            }
        }
    }

    // PRE-PASS 1d — `DEFINE PARAM` defaults (see `global_param_defaults`).
    let global_param_defaults = global_param_defaults(sources, &global_defined);

    // PRE-PASS 2 — implicit schemaless tables. Writing to (CREATE/UPSERT/
    // INSERT/DELETE) or hanging DDL (`DEFINE FIELD/EVENT/INDEX ... ON`) on a
    // never-`DEFINE`d table is valid SurrealQL: the table is created on
    // demand. Register an empty `TableDef` for each such target so those
    // positions resolve and downstream field checks stay lenient. A table
    // that is only ever READ and never defined stays unknown, so 1001 keeps
    // firing there.
    let mut implicit_tables: Vec<TableDef> = Vec::new();
    let mut seen_implicit: BTreeSet<String> = BTreeSet::new();
    for (parsed, statements) in sources {
        for stmt in statements {
            for (name, range) in implicit_table_targets(stmt) {
                if global_defined.tables.contains_key(&name) || !seen_implicit.insert(name.clone())
                {
                    continue;
                }
                implicit_tables.push(TableDef {
                    name: name.clone(),
                    source: parsed.source_id().clone(),
                    name_span: SourceSpan::new(parsed.source_id().clone(), range),
                    fields: BTreeMap::new(),
                    indexes: BTreeMap::new(),
                    events: BTreeMap::new(),
                    relation: None,
                    schemafull: false,
                    drop_table: false,
                    changefeed: false,
                });
            }
        }
    }

    // W5009 guardedness: whether each `fn::` body branches (any IF/FOR). A
    // recursion cycle only provably never terminates when no function in the
    // cycle can branch to a base case. Computed here in source order so a later
    // same-named redefinition wins, matching the per-source hoist below.
    let mut fn_guarded: BTreeMap<String, bool> = BTreeMap::new();
    for (parsed, statements) in sources {
        for stmt in statements {
            if let Some(function) =
                crate::schema::extract_function_def(stmt, parsed.source_id(), parsed.text())
            {
                fn_guarded.insert(function.name.clone(), fn_body_branches(stmt));
            }
        }
    }

    GlobalCatalog {
        target_version: None,
        global_defined,
        global_fn_returns,
        global_field_kinds,
        global_param_defaults,
        implicit_tables,
        fn_guarded,
    }
}

/// PRE-PASS 1d — every `DEFINE PARAM`'s default fact, keyed by param name.
///
/// A `DEFINE PARAM $x VALUE …` installs the value database-side, so `$x` is
/// neither unknown nor host-required in ANY source — not just in the ones that
/// textually follow the definition. The per-source `StatementEnv` resets
/// between sources, so recording the default only during the walk (which still
/// happens, for the within-source ordering contract 6002) left every
/// cross-source reader with `unknown [required]`: a generated host adapter
/// demanding a param the database already supplies.
///
/// Inferred against the full catalog so a `VALUE` reading a table or a `fn::`
/// helper resolves, and in source order, so a later redefinition wins.
fn global_param_defaults(
    sources: &[LoweredSource<'_>],
    catalog: &SchemaIndex,
) -> BTreeMap<String, crate::expression::ExpressionFact> {
    let mut defaults = BTreeMap::new();
    for (parsed, statements) in sources {
        for stmt in statements {
            let ast::Statement::Define(ast::DefineStmt::Param(def)) = &stmt.node else {
                continue;
            };
            let Some(value) = &def.value else {
                continue;
            };
            let mut scratch: Vec<Finding> = Vec::new();
            let mut ctx = AnalysisContext::new(
                catalog,
                parsed.source_id().clone(),
                parsed.text(),
                &mut scratch,
            );
            let fact = crate::analyzer::expression::expr_fact(&mut ctx, value);
            defaults.insert(def.name.node.clone(), fact);
        }
    }
    defaults
}

/// Layers the globally-computed inferences (PRE-PASS 1b/1c) onto a source's
/// working catalog: a caller in this source resolves a table-bearing UDF's
/// return type, and reads an untyped field's value kind, without any per-source
/// re-inference.
fn apply_global_inferences(global: &GlobalCatalog, working: &mut SchemaIndex) {
    for (name, kind) in &global.global_fn_returns {
        if let Some(function) = working.functions.get_mut(name) {
            if function.return_kind.is_none() {
                function.inferred_return = Some(kind.clone());
            }
        }
    }
    for ((table, field_key), kind) in &global.global_field_kinds {
        if let Some(field) = working
            .tables
            .get_mut(table)
            .and_then(|t| t.fields.get_mut(field_key))
        {
            if field.kind.is_none() {
                field.kind = Some(kind.clone());
                field.partial.clear();
            }
        }
    }
}

/// Runs the per-source walk for ONE source against a prebuilt [`GlobalCatalog`].
///
/// `working` is the source's *cross-source* catalog on entry — every OTHER
/// source's additive `DEFINE`s (the whole-workspace walk excludes the source's
/// own so within-source ordering contracts keep behaving; single-source
/// re-analysis passes `global.global_defined` directly, valid only because such
/// a source adds nothing to it). This function then layers on the implicit
/// tables, the within-source `fn::` hoist, and the globally-computed function
/// returns / field kinds, exactly as the whole-workspace loop did, and walks
/// the statements. Findings append to `diagnostics`; schema effects apply to
/// `working` and, when `schema_sink` is `Some`, to the run-wide schema too.
fn analyze_source_against(
    global: &GlobalCatalog,
    parsed: &ParsedSource,
    statements: &[ast::Spanned<ast::Statement>],
    mut working: SchemaIndex,
    diagnostics: &mut Vec<Finding>,
    mut schema_sink: Option<&mut SchemaIndex>,
) -> SourceAnalysis {
    for table in &global.implicit_tables {
        working.insert_table(table.clone(), false);
    }
    hoist_functions(parsed, statements, &mut working, diagnostics);

    // Reuse the globally-computed function returns and untyped-field value
    // kinds (PRE-PASS 1b/1c).
    apply_global_inferences(global, &mut working);

    let mut source_analysis = SourceAnalysis::default();
    let mut analyzer_env = StatementEnv::default();
    // Seed the engine-supplied session params (`$auth` as `option<record>`,
    // `$token`/`$session`/`$access`/`$scope`) as bound facts, so a
    // top-level `$auth` is never reported as a host param and guards can
    // narrow it.
    analyzer_env.seed_session_params(parsed.source_id());
    // Seed the workspace's `DEFINE PARAM` defaults (PRE-PASS 1d) so a reader
    // in ANY source carries the declared kind and is not host-required. The
    // per-statement walk re-records the definition it passes over, which keeps
    // the within-source ordering contract (6002) reading the same facts.
    for (name, fact) in &global.global_param_defaults {
        analyzer_env.define_param_default(name.clone(), fact.clone());
    }
    // Transaction pairing (4007): BEGIN opens exactly one transaction
    // that COMMIT/CANCEL closes.
    let mut open_transaction: Option<ByteRange> = None;

    for lowered in statements {
        let kind = {
            let mut ctx = AnalysisContext::scoped(
                &working,
                parsed.source_id().clone(),
                parsed.text(),
                diagnostics,
                analyzer_env,
                None,
            )
            // Order-independent existence checks (a `record<T>` target) ask the
            // whole-workspace catalog, not `working`, which by construction
            // holds only what precedes this statement.
            .with_workspace_catalog(&global.global_defined)
            .with_target_version(global.target_version);
            let kind = crate::analyzer::statement::analyze_lowered_statement(&mut ctx, lowered);
            // Syntax the configured target lacks (8003) or removed (8002), and
            // an OMIT with nothing to omit from (4012): both are shape checks
            // over the whole statement tree, run once per top-level statement.
            crate::analyzer::version::check_statement(&mut ctx, lowered);
            crate::analyzer::omit::check_statement(&mut ctx, lowered);
            // A top-level guard that exits (`IF $x = NONE THEN THROW … END;`)
            // narrows `$x` for every statement after it, exactly as it does
            // inside a block. Without this the source's statement sequence was
            // the one statement-sequence level that dropped the narrowing.
            crate::analyzer::flow::block::apply_fall_through_narrowing(&mut ctx, lowered);
            analyzer_env = ctx.into_env();
            kind
        };

        source_analysis
            .statements
            .push(statement_analysis(parsed.source_id(), lowered, kind));

        let span = || SourceSpan::new(parsed.source_id().clone(), lowered.span);
        match &lowered.node {
            ast::Statement::Begin(_) => {
                if open_transaction.is_some() {
                    diagnostics.push(surrealql_analyzer_diagnostics::catalog::finding(
                        span(),
                        4007,
                        "BEGIN inside an open transaction; transactions do not nest".to_string(),
                    ));
                }
                open_transaction = Some(lowered.span);
            }
            ast::Statement::Commit(_) | ast::Statement::Cancel(_)
                if open_transaction.take().is_none() =>
            {
                diagnostics.push(surrealql_analyzer_diagnostics::catalog::finding(
                    span(),
                    4007,
                    "COMMIT/CANCEL without an open BEGIN".to_string(),
                ));
            }
            _ => {}
        }

        // Apply the statement's effects to both this source's working
        // catalog (so later statements in the same file see them, and
        // REMOVE stays order-sensitive) and, when present, the run-wide
        // catalog returned to consumers.
        crate::schema::apply_schema_statement_effects(
            lowered,
            parsed.source_id(),
            parsed.text(),
            &mut working,
            Some(&global.global_defined),
        );
        if let Some(schema) = schema_sink.as_deref_mut() {
            crate::schema::apply_schema_statement_effects(
                lowered,
                parsed.source_id(),
                parsed.text(),
                schema,
                Some(&global.global_defined),
            );
        }
    }

    if let Some(open_span) = open_transaction {
        diagnostics.push(surrealql_analyzer_diagnostics::catalog::finding(
            SourceSpan::new(parsed.source_id().clone(), open_span),
            4007,
            "this BEGIN is never closed; add COMMIT or CANCEL".to_string(),
        ));
    }

    // A parameter read before its LET in source order sees nothing (6004).
    for param in analyzer_env.params() {
        if let Some(binding) = analyzer_env.let_fact(&param.name) {
            for use_span in &param.spans {
                if use_span.source() == binding.span.source()
                    && use_span.range().start() < binding.span.range().start()
                {
                    diagnostics.push(surrealql_analyzer_diagnostics::catalog::finding(
                        use_span.clone(),
                        6004,
                        format!("`${}` is read before its LET on this line runs", param.name),
                    ));
                }
            }
        }
    }

    // Unused LET bindings (7001): a `LET $x` never read in the rest of its
    // scope. Opt-in (catalog default `allow`), so it is filtered from the
    // default oracle by policy resolution; emitted here as a raw finding.
    crate::analyzer::flow::unused_let::check_unused_lets(
        statements,
        parsed.source_id(),
        parsed.text(),
        diagnostics,
    );

    // Recording already skipped LET-bound uses, so everything left is
    // host-supplied — including forward uses that a later LET shadows.
    source_analysis.params = analyzer_env.params();
    // Every LET/FOR binding recorded during the walk (all nesting depths), and
    // every guard-narrowed region, so an editor can answer per occurrence.
    (source_analysis.let_bindings, source_analysis.narrowings) = analyzer_env.editor_facts();
    source_analysis
}

/// The per-source walk output for a single source: its analysis record plus
/// every finding spanned in it.
pub(crate) struct OneSourceOutput {
    pub analysis: SourceAnalysis,
    pub diagnostics: Vec<Finding>,
}

/// Re-analyzes ONE source against a prebuilt [`GlobalCatalog`], reproducing the
/// output the whole-workspace walk would produce for that source — WITHOUT
/// rebuilding the schema or touching any other source.
///
/// Soundness contract: the source must contribute nothing to `global` — no
/// additive `DEFINE`, no implicit-table target, no `fn::` definition — so that
/// `global.global_defined` already equals "every other source's" catalog for
/// it. Callers (the LSP fast path) gate on that; a source that fails it must be
/// analyzed through the full [`analyze_sources_with`] pass instead. The
/// cross-source function-cycle check (5009) is intentionally skipped: it spans
/// findings at a function's own definition, and a qualifying source defines no
/// functions, so it never owns one.
pub(crate) fn analyze_one_source(
    global: &GlobalCatalog,
    parsed: &ParsedSource,
    require_suppression_reasons: bool,
) -> OneSourceOutput {
    let statements = surrealql_analyzer_syntax::lower::lower_statements(parsed);
    let working = global.global_defined.clone();
    let mut diagnostics = Vec::new();
    let analysis =
        analyze_source_against(global, parsed, &statements, working, &mut diagnostics, None);

    // Suppression runs last so directives can silence every finding kind, just
    // as the whole-workspace pass applies it per source at the end.
    if parsed.syntax_diagnostics().is_empty() {
        crate::suppress::apply_suppressions(
            parsed.source_id(),
            parsed.text(),
            require_suppression_reasons,
            &mut diagnostics,
        );
    }

    OneSourceOutput {
        analysis,
        diagnostics,
    }
}

/// The cross-source cycle findings implied by a catalog — `fn::` recursion
/// (5009) and event self-triggering (5010) — spanned at each offending
/// definition. Computed purely from the catalog (the function call graph plus
/// guardedness; the event write graph), so it can be derived once and reused
/// across the symbol-incremental path — both to attribute a cycle change to the
/// sources it touches and to inject the findings into each re-analyzed defining
/// source.
pub(crate) fn function_cycle_findings(global: &GlobalCatalog) -> Vec<Finding> {
    let mut diagnostics = Vec::new();
    check_function_cycles(&global.global_defined, &global.fn_guarded, &mut diagnostics);
    check_event_cycles(&global.global_defined, &mut diagnostics);
    diagnostics
}

/// Re-analyzes ONLY the `affected` sources against a prebuilt [`GlobalCatalog`],
/// reproducing the exact per-source output the whole-workspace walk
/// ([`analyze_sources_with`]) would produce for each — without re-walking the
/// unaffected sources.
///
/// Unlike [`analyze_one_source`] (whose contract requires the source to
/// contribute nothing to the catalog), this reproduces the whole-workspace
/// loop's per-source `working` base for EVERY affected source, schema or query:
/// the catalog's additive definitions MINUS the source's own, so within-source
/// ordering contracts (duplicate definition, REMOVE) behave identically. It also
/// injects the source's own 5009 cross-source cycle findings, so an affected
/// *schema* source that defines a recursive function reproduces those too.
///
/// Soundness rests on the caller's `affected` set being a superset of every
/// source whose output could differ from the previous catalog state — see
/// [`crate::analysis::changed_symbols`] /
/// [`crate::analysis::source_reference_set`] and the cross-source cycle
/// attribution in [`crate::analysis::sources_with_changed_cycle_findings`].
pub(crate) fn reanalyze_sources<P: std::borrow::Borrow<ParsedSource>>(
    parsed_sources: &[P],
    global: &GlobalCatalog,
    affected: &BTreeSet<SourceId>,
    require_suppression_reasons: bool,
) -> BTreeMap<SourceId, OneSourceOutput> {
    let sources = lower_all(parsed_sources);

    // The whole 5009 finding set implied by the catalog, spanned in each
    // offending function's defining source. Each affected source below claims
    // the findings spanned in it, matching the whole-workspace pass which runs
    // the cycle check once and distributes its findings by span.
    let cycle_findings = function_cycle_findings(global);

    let mut out = BTreeMap::new();
    for (index, (parsed, statements)) in sources.iter().enumerate() {
        if !affected.contains(parsed.source_id()) {
            continue;
        }

        // Reproduce the whole-workspace loop's per-source base: every OTHER
        // source's additive definitions. The source's own definitions
        // accumulate during its walk, preserving within-source ordering
        // contracts exactly as `analyze_sources_with` does.
        let mut working = SchemaIndex::default();
        for (other_index, (other, other_statements)) in sources.iter().enumerate() {
            if other_index == index {
                continue;
            }
            for stmt in other_statements {
                apply_additive_define(stmt, other.source_id(), other.text(), &mut working);
            }
        }

        let mut diagnostics = Vec::new();
        let analysis =
            analyze_source_against(global, parsed, statements, working, &mut diagnostics, None);

        // This source's share of the cross-source cycle findings (5009).
        for finding in &cycle_findings {
            if finding.span().source() == parsed.source_id() {
                diagnostics.push(finding.clone());
            }
        }

        // Suppression runs last, exactly as the whole-workspace pass applies it
        // per source at the end (covering the injected 5009 findings too).
        if parsed.syntax_diagnostics().is_empty() {
            crate::suppress::apply_suppressions(
                parsed.source_id(),
                parsed.text(),
                require_suppression_reasons,
                &mut diagnostics,
            );
        }

        out.insert(
            parsed.source_id().clone(),
            OneSourceOutput {
                analysis,
                diagnostics,
            },
        );
    }
    out
}

/// Rebuilds the run-wide [`SchemaIndex`] exactly as [`analyze_sources_with`]
/// does — sequential [`apply_schema_statement_effects`] over every source's
/// statements in order — but WITHOUT the per-statement type-inference walk. The
/// schema is the cheap part of a full pass; the symbol-incremental LSP path uses
/// this to reproduce `analyze_workspace`'s `schema` when a schema edit made the
/// cached one stale, so editor features keep resolving against the current
/// catalog without re-analyzing every source.
///
/// [`apply_schema_statement_effects`]: crate::schema::apply_schema_statement_effects
pub(crate) fn build_run_schema<P: std::borrow::Borrow<ParsedSource>>(
    parsed_sources: &[P],
) -> SchemaIndex {
    let sources = lower_all(parsed_sources);
    // PRE-PASS 1 only (the additive union — no inference), so the order-
    // independent lookups inside `apply_schema_statement_effects` see the same
    // whole-workspace catalog the full pass gives them. Skipping it here would
    // make this rebuild disagree with `analyze_workspace`'s `schema` for a
    // `COMPUTED <~t` whose target is declared later.
    let workspace = build_global_defined(&sources);
    let mut schema = SchemaIndex::default();
    for (parsed, statements) in &sources {
        for stmt in statements {
            crate::schema::apply_schema_statement_effects(
                stmt,
                parsed.source_id(),
                parsed.text(),
                &mut schema,
                Some(&workspace),
            );
        }
    }
    schema
}

pub(crate) fn analyze_sources(parsed_sources: &[ParsedSource]) -> PipelineOutput {
    analyze_sources_with(parsed_sources, false, None)
}

pub(crate) fn analyze_sources_with(
    parsed_sources: &[ParsedSource],
    require_suppression_reasons: bool,
    target_version: Option<TargetVersion>,
) -> PipelineOutput {
    let mut output = PipelineOutput::default();

    // Lower every source once and run the order-independent pre-passes into the
    // reusable global catalog. Single-source re-analysis consumes the same
    // `build_global_catalog` output, so the two paths can never diverge.
    let sources = lower_all(parsed_sources);
    let global = build_global_catalog_from_lowered(&sources).with_target_version(target_version);

    for (index, (parsed, statements)) in sources.iter().enumerate() {
        // The catalog this source is analyzed against: every OTHER source's
        // additive definitions (cross-file visibility). This source's own
        // definitions accumulate incrementally during the walk, so
        // within-source ordering contracts (duplicate definition, REMOVE) keep
        // behaving exactly as before. (The single-source path passes
        // `global.global_defined` directly, valid only because a qualifying
        // source contributes nothing to it.)
        let mut working = SchemaIndex::default();
        for (other_index, (other, other_statements)) in sources.iter().enumerate() {
            if other_index == index {
                continue;
            }
            for stmt in other_statements {
                apply_additive_define(stmt, other.source_id(), other.text(), &mut working);
            }
        }

        let source_analysis = analyze_source_against(
            &global,
            parsed,
            statements,
            working,
            &mut output.diagnostics,
            Some(&mut output.schema),
        );
        output
            .sources
            .insert(parsed.source_id().clone(), source_analysis);
    }

    // fn:: definitions must terminate: an unconditional direct or mutual
    // recursion cycle never does (5009). Three-color DFS; each cycle reports
    // once, and only when no function in it can branch to a base case.
    check_function_cycles(&output.schema, &global.fn_guarded, &mut output.diagnostics);
    // Events must not trigger themselves: an event whose body writes a table
    // whose event writes back — or its own table — can fire again (5010).
    check_event_cycles(&output.schema, &mut output.diagnostics);

    // Suppression runs last so directives can silence every finding kind,
    // including the cross-source passes above.
    for parsed in parsed_sources {
        if parsed.syntax_diagnostics().is_empty() {
            crate::suppress::apply_suppressions(
                parsed.source_id(),
                parsed.text(),
                require_suppression_reasons,
                &mut output.diagnostics,
            );
        }
    }

    output
}

/// The per-statement record consumers see: its span, a stable kind name,
/// the response kind where the statement has one, and SELECT's clause
/// modifiers.
fn statement_analysis(
    source: &SourceId,
    lowered: &ast::Spanned<ast::Statement>,
    kind: Kind,
) -> StatementAnalysis {
    use ast::Statement as S;
    let (name, has_response): (&str, bool) = match &lowered.node {
        S::Select(_) => ("select", true),
        S::Create(_) => ("create", true),
        S::Update(_) => ("update", true),
        S::Upsert(_) => ("upsert", true),
        S::Delete(_) => ("delete", true),
        S::Insert(_) => ("insert", true),
        S::Relate(_) => ("relate", true),
        S::Let(_) => ("let", false),
        S::Return(_) => ("return", true),
        S::IfElse(_) => ("if_else", true),
        S::For(_) => ("for", false),
        S::Block(_) => ("block", true),
        S::Throw(_) => ("throw", false),
        S::Break(_) => ("break", false),
        S::Continue(_) => ("continue", false),
        S::Begin(_) => ("begin", false),
        S::Commit(_) => ("commit", false),
        S::Cancel(_) => ("cancel", false),
        S::Define(stmt) => (
            match stmt {
                ast::DefineStmt::Table(_) => "define_table",
                ast::DefineStmt::Field(_) => "define_field",
                ast::DefineStmt::Index(_) => "define_index",
                ast::DefineStmt::Event(_) => "define_event",
                ast::DefineStmt::Param(_) => "define_param",
                ast::DefineStmt::Function(_) => "define_function",
                ast::DefineStmt::Analyzer(_) => "define_analyzer",
                ast::DefineStmt::Other(_) => "define",
            },
            false,
        ),
        S::Remove(_) => ("remove", false),
        S::Alter(_) => ("alter", false),
        S::Info(_) => ("info_for", false),
        S::Use(_) => ("use", false),
        S::Option(_) => ("option", false),
        S::Sleep(_) => ("sleep", false),
        S::Show(_) => ("show", true),
        S::Rebuild(_) => ("rebuild", false),
        S::LiveSelect(_) => ("live_select", true),
        S::Kill(_) => ("kill", false),
        S::Expr(_) => ("expression", true),
        S::Partial(_) => ("unknown", false),
    };

    let select_modifiers = match &lowered.node {
        S::Select(stmt) => select_modifiers(source, stmt),
        _ => Vec::new(),
    };

    StatementAnalysis {
        span: SourceSpan::new(source.clone(), lowered.span),
        kind: name.to_string(),
        response_kind: has_response.then_some(kind),
        select_modifiers,
    }
}

/// SELECT's clause modifiers, with row-preservation semantics: WHERE and
/// friends keep the row shape; GROUP/SPLIT/EXPLAIN change it.
fn select_modifiers(source: &SourceId, stmt: &ast::SelectStmt) -> Vec<SelectModifierAnalysis> {
    let span =
        |range: surrealql_analyzer_syntax::span::ByteRange| SourceSpan::new(source.clone(), range);
    let modifier = |kind: &str, range, row_preserving, max_len| SelectModifierAnalysis {
        kind: kind.to_string(),
        span: span(range),
        row_preserving,
        max_len,
    };

    let mut out = Vec::new();
    if let Some(cond) = &stmt.where_clause {
        out.push(modifier("where", cond.span, true, None));
    }
    if let Some(order) = &stmt.order {
        if let Some(first) = order.keys.first() {
            out.push(modifier("order", first.expr.span, true, None));
        }
    }
    if let Some(limit) = &stmt.limit {
        let max_len = crate::analyzer::data::select::constant_row_limit(&limit.node);
        out.push(modifier("limit", limit.span, true, max_len));
    }
    if let Some(start) = &stmt.start {
        out.push(modifier("start", start.span, true, None));
    }
    if let Some(timeout) = &stmt.timeout {
        out.push(modifier("timeout", timeout.span, true, None));
    }
    if let Some(parallel) = stmt.parallel {
        out.push(modifier("parallel", parallel, true, None));
    }
    if let Some(group) = &stmt.group {
        let range = group.keys.first().map_or(
            surrealql_analyzer_syntax::span::ByteRange::new(0, 0).expect("ordered"),
            |key| key.span,
        );
        out.push(modifier("group", range, false, None));
    }
    if let Some(first) = stmt.split.first() {
        out.push(modifier("split", first.span, false, None));
    }
    if let Some(explain) = stmt.explain {
        out.push(modifier("explain", explain, false, None));
    }
    out
}

/// Applies one lowered statement's *additive* `DEFINE` effect to `schema`,
/// reusing schema.rs's extraction. Unlike
/// [`crate::schema::apply_schema_statement_effects`], this skips `REMOVE` and
/// `ALTER` (order-sensitive, owned by the walk) — it exists to pre-build the
/// order-independent global namespace.
fn apply_additive_define(
    stmt: &ast::Spanned<ast::Statement>,
    source: &SourceId,
    text: &str,
    schema: &mut SchemaIndex,
) {
    let ast::Statement::Define(def) = &stmt.node else {
        return;
    };
    match def {
        ast::DefineStmt::Table(def) => {
            schema.insert_table(
                crate::schema::table_def_from_ast(def, source),
                def.overwrite,
            );
        }
        ast::DefineStmt::Field(def) => {
            schema.insert_field(
                crate::schema::field_def_from_ast(def, source, text),
                def.overwrite,
            );
        }
        ast::DefineStmt::Index(def) => {
            schema.insert_index(crate::schema::index_def_from_ast(def, source));
        }
        ast::DefineStmt::Event(def) => {
            schema.insert_event(crate::schema::event_def_from_ast(def, source));
        }
        ast::DefineStmt::Param(def) => {
            schema.insert_param(crate::schema::param_def_from_ast(def, source));
        }
        ast::DefineStmt::Function(def) => {
            schema.insert_function(crate::schema::function_def_from_ast(
                def, source, text, stmt.span,
            ));
        }
        ast::DefineStmt::Analyzer(def) => {
            schema.insert_analyzer(crate::schema::analyzer_def_from_ast(def, source));
        }
        ast::DefineStmt::Other(_) => {}
    }
}

/// The tables a statement implicitly creates on demand: write targets
/// (CREATE/UPSERT/INSERT/DELETE) and a `DEFINE FIELD ... ON` target. Writing
/// to, or hanging a field on, a never-`DEFINE`d table is valid SurrealQL —
/// the table exists schemaless.
///
/// RELATE edges are excluded: an undefined edge is a relation-contract
/// question (3001/3002), not an implicit plain table. `DEFINE EVENT`/`DEFINE
/// INDEX` `ON` targets are excluded because their downstream field checks
/// (in the event/index analyzers) would then run against the empty implicit
/// table and report an *unknown field* (1002) instead — trading one false
/// positive for another. A field/event/index on an undefined table therefore
/// still reports, which no valid corpus exercises.
fn implicit_table_targets(stmt: &ast::Spanned<ast::Statement>) -> Vec<(String, ByteRange)> {
    fn target(source: Option<&ast::Spanned<ast::Expr>>) -> Option<(String, ByteRange)> {
        let expr = source?;
        let name = crate::analyzer::data::mutation::source_table_name(source)?;
        Some((name, expr.span))
    }

    let mut out = Vec::new();
    match &stmt.node {
        ast::Statement::Create(stmt) => out.extend(target(stmt.targets.first())),
        ast::Statement::Upsert(stmt) => out.extend(target(stmt.targets.first())),
        ast::Statement::Delete(stmt) => out.extend(target(stmt.targets.first())),
        ast::Statement::Insert(stmt) => out.extend(target(stmt.target.as_ref())),
        ast::Statement::Define(ast::DefineStmt::Field(def)) => {
            out.push((def.table.node.clone(), def.table.span));
        }
        _ => {}
    }
    out
}

/// Whether a `DEFINE FUNCTION` body contains any branching — an `IF` or a
/// `FOR`, i.e. a construct that can route around a self-call to a base case.
///
/// Used to keep 5009 to provably-degenerate recursion: a guarded self-call may
/// terminate and must not be flagged. The carve-out is load-bearing (see
/// `docs/plans/2026-07-25-analyzer-gap-backlog.md` DX-12), so this walks the
/// lowered body rather than guessing at it.
///
/// It used to guess. The predicate was
/// `has_keyword(body_text, "if") || has_keyword(body_text, "for")` over the raw
/// lowercased source, with no string- or comment-awareness, so
/// `DEFINE FUNCTION fn::x() { RETURN 'if you see this'; }` was classified as
/// branching and `-- for now` in a comment was too. Both directions were wrong:
/// a body that only *mentions* the word was spared 5009, and the span it
/// scanned started at the end of the function *name*, so a parameter named
/// `$if_true` counted as a branch.
fn fn_body_branches(stmt: &ast::Spanned<ast::Statement>) -> bool {
    let ast::Statement::Define(ast::DefineStmt::Function(def)) = &stmt.node else {
        return false;
    };
    def.body.as_ref().is_some_and(block_branches)
}

/// Whether any statement in a block branches.
fn block_branches(block: &ast::Block) -> bool {
    block
        .statements
        .iter()
        .any(|statement| statement_branches(&statement.node))
}

/// Whether a statement is, or contains, an `IF` or a `FOR`.
///
/// Recursive rather than a flat match: a branch nested inside another block, a
/// `LET` whose value is an `IF`-expression, and a `RETURN IF … ELSE …` are all
/// branches, and the text scan happened to catch every one of them for the
/// wrong reason.
fn statement_branches(stmt: &ast::Statement) -> bool {
    match stmt {
        ast::Statement::IfElse(_) | ast::Statement::For(_) => true,
        ast::Statement::Block(block) => block_branches(block),
        ast::Statement::Let(let_stmt) => expr_branches(&let_stmt.value.node),
        ast::Statement::Return(ret) => ret
            .value
            .as_ref()
            .is_some_and(|value| expr_branches(&value.node)),
        ast::Statement::Throw(throw) => throw
            .value
            .as_ref()
            .is_some_and(|value| expr_branches(&value.node)),
        ast::Statement::Expr(expr) => expr_branches(&expr.node),
        _ => false,
    }
}

/// Whether an expression carries a branch: the forms that hold statements, plus
/// the operands of the forms that hold expressions.
fn expr_branches(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Block(block) => block_branches(block),
        ast::Expr::Subquery(inner) => statement_branches(&inner.node),
        ast::Expr::Binary { lhs, rhs, .. } => expr_branches(&lhs.node) || expr_branches(&rhs.node),
        ast::Expr::Prefix { expr, .. } | ast::Expr::Cast { expr, .. } => expr_branches(&expr.node),
        ast::Expr::Call(call) => call.args.iter().any(|arg| expr_branches(&arg.node)),
        ast::Expr::Array(items) => items.iter().any(|item| expr_branches(&item.node)),
        ast::Expr::Object(entries) => entries.iter().any(|(_, value)| expr_branches(&value.node)),
        // A closure body is a branch the caller can route through, exactly as
        // an inline `IF` is.
        ast::Expr::Closure(closure) => expr_branches(&closure.body.node),
        _ => false,
    }
}

fn check_function_cycles(
    schema: &SchemaIndex,
    fn_guarded: &BTreeMap<String, bool>,
    diagnostics: &mut Vec<Finding>,
) {
    fn find_cycle(
        name: &str,
        functions: &BTreeMap<String, crate::schema::FunctionDef>,
        gray: &mut Vec<String>,
        black: &mut BTreeSet<String>,
    ) -> Option<Vec<String>> {
        if black.contains(name) {
            return None;
        }
        if let Some(position) = gray.iter().position(|entry| entry == name) {
            return Some(gray[position..].to_vec());
        }
        gray.push(name.to_string());
        if let Some(def) = functions.get(name) {
            for callee in &def.callees {
                if let Some(cycle) = find_cycle(callee, functions, gray, black) {
                    return Some(cycle);
                }
            }
        }
        gray.pop();
        black.insert(name.to_string());
        None
    }

    let mut black = BTreeSet::new();
    let mut on_reported_cycle = BTreeSet::new();
    for function in schema.functions.values() {
        if black.contains(&function.name) || on_reported_cycle.contains(&function.name) {
            continue;
        }
        let mut gray = Vec::new();
        if let Some(cycle) = find_cycle(&function.name, &schema.functions, &mut gray, &mut black) {
            for name in &cycle {
                on_reported_cycle.insert(name.clone());
            }
            // Only provably-degenerate recursion is flagged: if any function
            // in the cycle can branch to a base case, the recursion may
            // terminate (bounded/tree recursion is legitimate) and is left
            // alone.
            let unconditional = cycle
                .iter()
                .all(|name| !fn_guarded.get(name).copied().unwrap_or(false));
            if unconditional {
                diagnostics.push(surrealql_analyzer_diagnostics::catalog::finding(
                    function.name_span.clone(),
                    5009,
                    format!(
                        "`{}` never terminates: {} calls itself",
                        function.name,
                        cycle.join(" -> "),
                    ),
                ));
            }
        }
    }
}

/// Events must not trigger themselves, directly or through other tables'
/// events (5010).
///
/// Nodes are events; an edge runs from event `a` to event `b` when `a`'s body
/// writes `b`'s table with a kind `b` fires on (its `WHEN` narrowed to
/// `$event = 'CREATE'` is not re-fired by an `UPDATE`). A cycle in that graph
/// is a write that can raise the event that performed it. Same three-color DFS
/// as [`check_function_cycles`]: each cycle reports once, on every event in it.
///
/// This is a "can", not a "will": a `WHEN` on the written fields, or a body
/// that only writes rows the condition then rejects, breaks the loop in ways
/// the catalog does not see. The contract is that an event body does not write
/// a table whose event fires on that write; the message says so.
fn check_event_cycles(schema: &SchemaIndex, diagnostics: &mut Vec<Finding>) {
    use crate::schema::EventDef;

    /// The graph key of an event: its table and name, in that order.
    type Node = (String, String);

    fn node(event: &EventDef) -> Node {
        (event.table.clone(), event.name.clone())
    }

    /// The events `event`'s body can fire, with the write that fires each.
    fn successors<'a>(
        event: &'a EventDef,
        schema: &'a SchemaIndex,
    ) -> impl Iterator<Item = (&'a EventDef, &'a crate::schema::EventWrite)> + 'a {
        event.writes.iter().flat_map(move |write| {
            schema
                .tables
                .get(&write.table)
                .into_iter()
                .flat_map(|table| table.events.values())
                .filter(move |target| write.kinds.intersects(target.triggers))
                .map(move |target| (target, write))
        })
    }

    fn find_cycle(
        event: &EventDef,
        schema: &SchemaIndex,
        gray: &mut Vec<Node>,
        black: &mut BTreeSet<Node>,
    ) -> Option<Vec<Node>> {
        let key = node(event);
        if black.contains(&key) {
            return None;
        }
        if let Some(position) = gray.iter().position(|entry| *entry == key) {
            return Some(gray[position..].to_vec());
        }
        gray.push(key.clone());
        for (next, _) in successors(event, schema) {
            if let Some(cycle) = find_cycle(next, schema, gray, black) {
                return Some(cycle);
            }
        }
        gray.pop();
        black.insert(key);
        None
    }

    let mut black = BTreeSet::new();
    let mut on_reported_cycle: BTreeSet<Node> = BTreeSet::new();
    for event in schema.events() {
        let key = node(event);
        if black.contains(&key) || on_reported_cycle.contains(&key) {
            continue;
        }
        let mut gray = Vec::new();
        let Some(cycle) = find_cycle(event, schema, &mut gray, &mut black) else {
            continue;
        };
        on_reported_cycle.extend(cycle.iter().cloned());

        let path = |from: usize| {
            cycle
                .iter()
                .cycle()
                .skip(from)
                .take(cycle.len() + 1)
                .map(|(table, name)| format!("{name} ON {table}"))
                .collect::<Vec<_>>()
                .join(" -> ")
        };
        for (position, (table, name)) in cycle.iter().enumerate() {
            let Some(current) = schema.tables.get(table).and_then(|t| t.events.get(name)) else {
                continue;
            };
            // The write that carries this event's step of the cycle: the one
            // whose target is the next event in it.
            let (next_table, next_name) = &cycle[(position + 1) % cycle.len()];
            let Some((next, write)) = successors(current, schema)
                .find(|(next, _)| next.table == *next_table && next.name == *next_name)
            else {
                continue;
            };
            let fired = write.kinds.intersect(next.triggers).names().join("/");
            let message = if cycle.len() == 1 {
                format!(
                    "`{name}` can trigger itself: its body writes `{table}` ({fired}), which \
                     fires `{name}` again"
                )
            } else {
                format!(
                    "`{name}` can trigger itself through other events: {}",
                    path(position)
                )
            };
            diagnostics.push(
                surrealql_analyzer_diagnostics::catalog::finding(
                    current.name_span.clone(),
                    5010,
                    message,
                )
                .with_help(
                    "surrealql-analyzer cannot see whether a WHEN condition or the written fields \
                         break the loop; narrow WHEN on `$event` (a CREATE-only event is not \
                         re-fired by its own UPDATE) or write a table without events",
                )
                .with_related(
                    write.span.clone(),
                    format!("`{}` is written here ({fired})", write.table),
                ),
            );
        }
    }
}

/// Hoists every `fn::` this source defines into `working` before the walk, so a
/// body may call a function defined later in the same file — and reports the
/// cross-source duplicates (1022) that the hoist would otherwise hide.
///
/// See the comment inside for why the report belongs here and not in the
/// `DEFINE FUNCTION` analyzer.
fn hoist_functions(
    parsed: &ParsedSource,
    statements: &[ast::Spanned<ast::Statement>],
    working: &mut SchemaIndex,
    diagnostics: &mut Vec<Finding>,
) {
    // Within-source fn:: hoist: a body may call functions defined later
    // in the same file.
    //
    // The hoist is also the reason a `fn::` defined in TWO sources used to go
    // unreported: `working` arrives holding every other source's `DEFINE`s, and
    // inserting this source's copy over the foreign one means the walk's own
    // 1022 check (`schema::define::function`) finds only this source's entry
    // and concludes there is nothing to report. A `DEFINE TABLE` has no hoist,
    // so its foreign twin survives and 1022 fires — from BOTH sources, since
    // each one's `working` holds the other's. Displacement is recorded here so
    // a duplicated `fn::` reports the same way, and the hoist keeps displacing:
    // what a body's call resolves against is unchanged.
    let mut displaced_functions = Vec::new();
    for stmt in statements {
        let Some(function) =
            crate::schema::extract_function_def(stmt, parsed.source_id(), parsed.text())
        else {
            continue;
        };
        if let ast::Statement::Define(ast::DefineStmt::Function(def)) = &stmt.node {
            // `OVERWRITE`/`IF NOT EXISTS` are deliberate redefinitions, exactly
            // as they are for the within-source case.
            if !def.overwrite && !def.if_not_exists {
                if let Some(existing) = working
                    .function(&def.name.node)
                    .filter(|existing| existing.source != *parsed.source_id())
                {
                    displaced_functions.push((
                        def.name.node.clone(),
                        def.name.span,
                        existing.name_span.clone(),
                    ));
                }
            }
        }
        working.insert_function(function);
    }
    if !displaced_functions.is_empty() {
        // Reported through the same helper the DEFINE analyzers use, so the
        // message, the help and the "defined here" related span cannot drift
        // from every other 1022.
        let mut ctx = AnalysisContext::scoped(
            working,
            parsed.source_id().clone(),
            parsed.text(),
            diagnostics,
            StatementEnv::default(),
            None,
        );
        for (name, name_span, existing) in displaced_functions {
            crate::analyzer::schema::define::emit_duplicate_definition(
                &mut ctx,
                name_span,
                &format!("`{name}`"),
                &format!("DEFINE FUNCTION OVERWRITE {name}(...)"),
                existing,
            );
        }
    }
}
