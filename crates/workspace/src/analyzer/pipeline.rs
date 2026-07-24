//! The workspace analysis pipeline: one source-order walk per source.
//!
//! Each top-level statement is lowered and dispatched to its analyzer
//! (which emits both its response kind and its findings), against the
//! schema built from the statements before it. The walk also owns the
//! cross-statement contracts that no single statement can see: schema
//! effects, transaction pairing (4007), parameter constraints and their
//! source-order rules (6004), and `fn::` termination (5009).
//!
//! The schema index is shared across the whole run and accumulates in
//! iteration order: a `DEFINE` in one source is visible to statements in
//! sources that come after it, never before. Parameter environments and
//! transaction state reset per source.
//!
//! Every stage consumes the lowered AST: statements arrive from
//! [`surrealguard_syntax::lower::lower_statements`], and schema effects apply
//! to those same lowered values.

use std::collections::{BTreeMap, BTreeSet};

use surrealdb_types::Kind;
use surrealguard_syntax::ast;
use surrealguard_syntax::parse::ParsedSource;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use crate::analysis::{
    LetBindingAnalysis, ParamInference, SelectModifierAnalysis, StatementAnalysis,
};
use crate::analyzer::context::AnalysisContext;
use crate::schema::{SchemaIndex, TableDef};
use crate::statement_env::StatementEnv;
use surrealguard_diagnostics::Finding;

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
}

pub(crate) fn analyze_sources(parsed_sources: &[ParsedSource]) -> PipelineOutput {
    analyze_sources_with(parsed_sources, false)
}

pub(crate) fn analyze_sources_with(
    parsed_sources: &[ParsedSource],
    require_suppression_reasons: bool,
) -> PipelineOutput {
    let mut output = PipelineOutput::default();

    // Lower every parse-clean source once; the pre-passes and the ordered
    // walk all reuse the same lowered statements.
    let sources: Vec<(&ParsedSource, Vec<ast::Spanned<ast::Statement>>)> = parsed_sources
        .iter()
        .filter(|parsed| parsed.syntax_diagnostics().is_empty())
        .map(|parsed| (parsed, surrealguard_syntax::lower::lower_statements(parsed)))
        .collect();

    // PRE-PASS 1 — the global additive catalog. A namespace is global: a
    // `DEFINE` in any source is a legitimate target for a reference in every
    // other source, regardless of file sort order. Only additive `DEFINE`
    // effects apply here; `REMOVE` and the order-sensitive contracts (4007,
    // 6004, duplicate definition) stay in the ordered walk below.
    let mut global_defined = SchemaIndex::default();
    for (parsed, statements) in &sources {
        for stmt in statements {
            apply_additive_define(stmt, parsed.source_id(), parsed.text(), &mut global_defined);
        }
    }

    // PRE-PASS 2 — implicit schemaless tables. Writing to (CREATE/UPSERT/
    // INSERT/DELETE) or hanging DDL (`DEFINE FIELD/EVENT/INDEX ... ON`) on a
    // never-`DEFINE`d table is valid SurrealQL: the table is created on
    // demand. Register an empty `TableDef` for each such target so those
    // positions resolve and downstream field checks stay lenient. A table
    // that is only ever READ and never defined stays unknown, so 1001 keeps
    // firing there.
    let mut implicit_tables: Vec<TableDef> = Vec::new();
    let mut seen_implicit: BTreeSet<String> = BTreeSet::new();
    for (parsed, statements) in &sources {
        for stmt in statements {
            for (name, range) in implicit_table_targets(stmt) {
                if global_defined.tables.contains_key(&name) || !seen_implicit.insert(name.clone()) {
                    continue;
                }
                implicit_tables.push(TableDef {
                    name: name.clone(),
                    source: parsed.source_id().clone(),
                    name_span: SourceSpan::new(parsed.source_id().clone(), range),
                    fields: BTreeMap::new(),
                    indexes: BTreeMap::new(),
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
    // cycle can branch to a base case.
    let mut fn_guarded: BTreeMap<String, bool> = BTreeMap::new();

    for (index, (parsed, statements)) in sources.iter().enumerate() {
        // The catalog this source is analyzed against: every OTHER source's
        // definitions (cross-file visibility) plus the implicit schemaless
        // tables. This source's own definitions accumulate incrementally
        // during the walk, so within-source ordering contracts (duplicate
        // definition, REMOVE) keep behaving exactly as before.
        let mut working = SchemaIndex::default();
        for (other_index, (other, other_statements)) in sources.iter().enumerate() {
            if other_index == index {
                continue;
            }
            for stmt in other_statements {
                apply_additive_define(stmt, other.source_id(), other.text(), &mut working);
            }
        }
        for table in &implicit_tables {
            working.insert_table(table.clone(), false);
        }
        // Within-source fn:: hoist: a body may call functions defined later
        // in the same file.
        for stmt in statements {
            if let Some(function) =
                crate::schema::extract_function_def(stmt, parsed.source_id(), parsed.text())
            {
                fn_guarded.insert(function.name.clone(), fn_body_branches(stmt, parsed.text()));
                working.insert_function(function);
            }
        }

        let mut source_analysis = SourceAnalysis::default();
        let mut analyzer_env = StatementEnv::default();
        // Transaction pairing (4007): BEGIN opens exactly one transaction
        // that COMMIT/CANCEL closes.
        let mut open_transaction: Option<ByteRange> = None;

        for lowered in statements {
            let kind = {
                let mut ctx = AnalysisContext::scoped(
                    &working,
                    parsed.source_id().clone(),
                    parsed.text(),
                    &mut output.diagnostics,
                    analyzer_env,
                    None,
                );
                let kind = crate::analyzer::statement::analyze_lowered_statement(&mut ctx, lowered);
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
                        output
                            .diagnostics
                            .push(surrealguard_diagnostics::catalog::finding(
                                span(),
                                4007,
                                "BEGIN inside an open transaction; transactions do not nest"
                                    .to_string(),
                            ));
                    }
                    open_transaction = Some(lowered.span);
                }
                ast::Statement::Commit(_) | ast::Statement::Cancel(_)
                    if open_transaction.take().is_none() =>
                {
                    output
                        .diagnostics
                        .push(surrealguard_diagnostics::catalog::finding(
                            span(),
                            4007,
                            "COMMIT/CANCEL without an open BEGIN".to_string(),
                        ));
                }
                _ => {}
            }

            // Apply the statement's effects to both this source's working
            // catalog (so later statements in the same file see them, and
            // REMOVE stays order-sensitive) and the run-wide catalog
            // returned to consumers.
            crate::schema::apply_schema_statement_effects(
                lowered,
                parsed.source_id(),
                parsed.text(),
                &mut working,
            );
            crate::schema::apply_schema_statement_effects(
                lowered,
                parsed.source_id(),
                parsed.text(),
                &mut output.schema,
            );
        }

        if let Some(open_span) = open_transaction {
            output
                .diagnostics
                .push(surrealguard_diagnostics::catalog::finding(
                    SourceSpan::new(parsed.source_id().clone(), open_span),
                    4007,
                    "this BEGIN is never closed; add COMMIT or CANCEL".to_string(),
                ));
        }

        // A parameter read before its LET in source order sees nothing
        // (6004).
        for param in analyzer_env.params() {
            if let Some(binding) = analyzer_env.let_fact(&param.name) {
                for use_span in &param.spans {
                    if use_span.source() == binding.span.source()
                        && use_span.range().start() < binding.span.range().start()
                    {
                        output
                            .diagnostics
                            .push(surrealguard_diagnostics::catalog::finding(
                                use_span.clone(),
                                6004,
                                format!(
                                    "`${}` is read before its LET on this line runs",
                                    param.name
                                ),
                            ));
                    }
                }
            }
        }

        // Recording already skipped LET-bound uses, so everything left is
        // host-supplied — including forward uses that a later LET shadows.
        source_analysis.params = analyzer_env.params();
        // Every LET/FOR binding recorded during the walk (all nesting
        // depths), drained up to this top-level env.
        source_analysis.let_bindings = analyzer_env.let_bindings().to_vec();
        output
            .sources
            .insert(parsed.source_id().clone(), source_analysis);
    }

    // fn:: definitions must terminate: an unconditional direct or mutual
    // recursion cycle never does (5009). Three-color DFS; each cycle reports
    // once, and only when no function in it can branch to a base case.
    check_function_cycles(&output.schema, &fn_guarded, &mut output.diagnostics);

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
    let span = |range: surrealguard_syntax::span::ByteRange| SourceSpan::new(source.clone(), range);
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
        let max_len = match &limit.node {
            ast::Expr::Literal(ast::Literal::Int(value)) => u64::try_from(*value).ok(),
            _ => None,
        };
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
            surrealguard_syntax::span::ByteRange::new(0, 0).expect("ordered"),
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
/// [`crate::schema::apply_schema_statement_effects`], this skips `REMOVE`
/// (order-sensitive, owned by the walk) and events (unmodeled) — it exists to
/// pre-build the order-independent global namespace.
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
            schema.insert_table(crate::schema::table_def_from_ast(def, source), def.overwrite);
        }
        ast::DefineStmt::Field(def) => {
            schema.insert_field(
                crate::schema::field_def_from_ast(def, source, text),
                def.overwrite,
            );
        }
        ast::DefineStmt::Index(def) => {
            let index = crate::schema::index_def_from_ast(def, source);
            if let Some(table) = schema.tables.get_mut(&index.table) {
                table.indexes.insert(index.name.clone(), index);
            }
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
        ast::DefineStmt::Event(_) | ast::DefineStmt::Other(_) => {}
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
fn implicit_table_targets(
    stmt: &ast::Spanned<ast::Statement>,
) -> Vec<(String, ByteRange)> {
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

/// Whether a `DEFINE FUNCTION` body contains any branching (an `IF` or `FOR`),
/// i.e. a construct that can route around a self-call to a base case. Used to
/// keep 5009 to provably-degenerate recursion: a guarded self-call may
/// terminate and must not be flagged.
fn fn_body_branches(stmt: &ast::Spanned<ast::Statement>, text: &str) -> bool {
    let ast::Statement::Define(ast::DefineStmt::Function(def)) = &stmt.node else {
        return false;
    };
    let body_start = def.name.span.end() as usize;
    let body_end = stmt.span.end() as usize;
    let Some(body) = text.get(body_start..body_end) else {
        return false;
    };
    let lower = body.to_ascii_lowercase();
    has_keyword(&lower, "if") || has_keyword(&lower, "for")
}

/// Whether `haystack` (already lowercased) contains `keyword` as a whole word.
fn has_keyword(haystack: &str, keyword: &str) -> bool {
    let bytes = haystack.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut from = 0;
    while let Some(pos) = haystack[from..].find(keyword) {
        let start = from + pos;
        let end = start + keyword.len();
        let before_ok = start == 0 || !is_ident(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_ident(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
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
                diagnostics.push(surrealguard_diagnostics::catalog::finding(
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
