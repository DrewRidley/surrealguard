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
use surrealguard_syntax::span::SourceSpan;

use crate::analysis::{ParamInference, SelectModifierAnalysis, StatementAnalysis};
use crate::analyzer::context::AnalysisContext;
use crate::schema::SchemaIndex;
use crate::statement_env::StatementEnv;
use surrealguard_diagnostics::Finding;

#[derive(Debug, Default)]
pub struct PipelineOutput {
    pub schema: SchemaIndex,
    pub diagnostics: Vec<Finding>,
    pub sources: BTreeMap<SourceId, SourceAnalysis>,
}

#[derive(Debug, Default)]
pub struct SourceAnalysis {
    pub statements: Vec<StatementAnalysis>,
    pub params: Vec<ParamInference>,
}

pub fn analyze_sources(parsed_sources: &[ParsedSource]) -> PipelineOutput {
    analyze_sources_with(parsed_sources, false)
}

pub fn analyze_sources_with(
    parsed_sources: &[ParsedSource],
    require_suppression_reasons: bool,
) -> PipelineOutput {
    let mut output = PipelineOutput::default();

    for parsed in parsed_sources {
        if !parsed.syntax_diagnostics().is_empty() {
            continue;
        }

        let statements = surrealguard_syntax::lower::lower_statements(parsed);

        // fn:: signatures hoist: a body references functions at invocation
        // time, so definitions later in the source are legitimate targets.
        for statement in &statements {
            if let Some(function) =
                crate::schema::extract_function_def(statement, parsed.source_id(), parsed.text())
            {
                output.schema.insert_function(function);
            }
        }

        let mut source_analysis = SourceAnalysis::default();
        let mut analyzer_env = StatementEnv::default();
        // Transaction pairing (4007): BEGIN opens exactly one transaction
        // that COMMIT/CANCEL closes.
        let mut open_transaction: Option<surrealguard_syntax::span::ByteRange> = None;

        for lowered in &statements {
            let kind = {
                let mut ctx = AnalysisContext::scoped(
                    &output.schema,
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
        output
            .sources
            .insert(parsed.source_id().clone(), source_analysis);
    }

    // fn:: definitions must terminate: direct or mutual recursion never
    // does (5009). Three-color DFS; each cycle reports once.
    check_function_cycles(&output.schema, &mut output.diagnostics);

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
        let range = group
            .keys
            .first()
            .map(|key| key.span)
            .unwrap_or(surrealguard_syntax::span::ByteRange::new(0, 0).expect("ordered"));
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

fn check_function_cycles(schema: &SchemaIndex, diagnostics: &mut Vec<Finding>) {
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
