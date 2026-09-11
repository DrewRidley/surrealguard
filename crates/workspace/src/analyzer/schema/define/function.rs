//! `DEFINE FUNCTION` analysis.
//!
//! The definition's contracts: it is defined once (1022), every `$param` its
//! body reads is a name something binds (6008), and the body must return what
//! the `->` arrow declares (2012). Parameters are bound with their declared
//! kinds so the body gets real analysis (the usual expression and statement
//! checks run inside it).

use std::collections::BTreeSet;

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;
use surrealql_analyzer_syntax::ast::visit::{walk_expr, walk_statement, Visitor};
use surrealql_analyzer_syntax::span::{ByteRange, SourceSpan};

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::contract::{term_kind, Contract, Position};
use crate::analyzer::facts::{eval, Bindings};
use crate::expression::{ExpressionFact, ExpressionValueClass};

/// Analyzes a `DEFINE FUNCTION` body with its parameters bound to their
/// declared kinds (untyped params bind as `Any`) and returns the body's
/// response kind. Returns `None` when the definition has no body.
///
/// Running the body here is what surfaces the usual expression/statement
/// diagnostics inside it, and the returned kind is both what the `-> T`
/// contract (2012) is checked against and what an untyped function's callers
/// infer (persisted on [`crate::schema::FunctionDef::inferred_return`]).
pub(crate) fn infer_function_body_kind(
    ctx: &mut AnalysisContext<'_>,
    def: &ast::DefineFunction,
) -> Option<Kind> {
    let body = def.body.as_ref()?;
    let kind = ctx.with_child_env(|ctx| {
        // Engine-supplied session params (`$auth`, ...) are available in every
        // `fn::` body. Seeding here also covers the throwaway return-inference
        // path (`schema::infer_untyped_return`), which builds a fresh env; the
        // params bound below override the seed if they collide (they can't —
        // `$auth` and friends are protected names).
        ctx.seed_session_params();
        for (name, ty) in &def.params {
            let kind = ty
                .as_ref()
                .and_then(|ty| crate::schema::kind_from_type_expr(&ty.node, ctx.source_text()).kind)
                .unwrap_or(Kind::Any);
            let span =
                surrealql_analyzer_syntax::span::SourceSpan::new(ctx.source().clone(), name.span);
            let mut fact = ExpressionFact::new(span, ExpressionValueClass::Variable);
            fact.kind = Some(kind);
            ctx.define_local(name.node.trim_start_matches('$').to_string(), fact);
        }
        crate::analyzer::flow::block::analyze_block(ctx, body)
    });
    Some(kind)
}

/// A function is defined once (1022).
///
/// Every `fn::` in a source is hoisted into the catalog before the walk (a
/// body may call a function defined later in the file), so the catalog entry
/// this statement sees may be *itself*, or a later same-named definition that
/// the hoist let win. Only an entry from another source, or from earlier in
/// this one, is a definition this statement replaces; the later twin reports
/// when the walk reaches it and finds this one in the catalog.
///
/// A function defined in two *files* is reported from both, the way a table
/// already was: the hoist still replaces a foreign entry (so a body's call
/// resolves against this source's definition), but it records the entry it
/// displaced, and `pipeline::hoist_functions` reports that through the same
/// helper this check uses.
fn check_duplicate_function(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineFunction) {
    if stmt.overwrite || stmt.if_not_exists {
        return;
    }
    let existing = ctx.schema().function(&stmt.name.node).and_then(|existing| {
        let earlier = existing.source != *ctx.source()
            || existing.name_span.range().start() < stmt.name.span.start();
        earlier.then(|| existing.name_span.clone())
    });
    if let Some(existing) = existing {
        super::emit_duplicate_definition(
            ctx,
            stmt.name.span,
            &format!("`{}`", stmt.name.node),
            &format!("DEFINE FUNCTION OVERWRITE {}(...)", stmt.name.node),
            existing,
        );
    }
}

/// The kind a body that provably returns one constant returns, or `None` when
/// it does not have that shape.
///
/// The single `RETURN <const>;` body is the whole of it on purpose. A body with
/// several exits is several positions, and pointing one 2012 at the arrow for
/// the union of what they return is the shape this check already has; making it
/// point at the offending `RETURN` is compositional checking, which needs the
/// bidirectional walk this stage does not build.
fn returned_constant_kind(def: &ast::DefineFunction) -> Option<Kind> {
    let body = def.body.as_ref()?;
    let [statement] = body.statements.as_slice() else {
        return None;
    };
    let ast::Statement::Return(ret) = &statement.node else {
        return None;
    };
    term_kind(&eval(&ret.value.as_ref()?.node, Bindings::NONE))
}

/// Every `$name` the body reads, and every name the body itself binds.
///
/// Collected from the AST rather than from the analyzer env because the env
/// merges a child scope's uses upward without saying which scope bound what,
/// and this check needs the distinction. The bound set is *scope-insensitive*
/// on purpose: a `LET` anywhere in the body exempts the name everywhere in it.
/// Reading a body `LET` too early is already 6004's contract, and the two
/// checks must not both fire on one mistake.
#[derive(Default)]
struct BodyParams {
    used: Vec<(String, ByteRange)>,
    bound: BTreeSet<String>,
}

/// A param name as the catalog keys it: no leading `$`.
fn bare(name: &str) -> &str {
    name.trim_start_matches('$')
}

/// Whether this source declares `DEFINE PARAM $name` anywhere in it, including
/// *after* the function under analysis.
///
/// `ctx.schema()` is the incrementally built index — it holds the statements
/// before this one plus every other source, which is what the ordering
/// contracts need, but it therefore cannot see a `DEFINE PARAM` later in this
/// same file. A param is resolved at *call* time, not at definition time:
/// engine-verified on 3.2.3 that
///
/// ```surql
/// DEFINE FUNCTION fn::later() { RETURN $laterparam; };
/// DEFINE PARAM $laterparam VALUE "defined-after";
/// RETURN fn::later();  -- 'defined-after'
/// ```
///
/// so a later declaration binds the name and must not be reported.
///
/// Re-parsing the source is why this is called only on the path that is about
/// to emit: a body with no suspect name never pays for it.
fn source_declares_param(ctx: &AnalysisContext<'_>, name: &str) -> bool {
    let Ok(parsed) =
        surrealql_analyzer_syntax::parse::parse_source(ctx.source().clone(), ctx.source_text())
    else {
        // Unparsable: claiming the name unbound would be inventing a fact.
        return true;
    };
    surrealql_analyzer_syntax::lower::lower_statements(&parsed)
        .iter()
        .any(|statement| {
            matches!(
                &statement.node,
                ast::Statement::Define(ast::DefineStmt::Param(param))
                    if bare(&param.name.node) == name
            )
        })
}

impl Visitor for BodyParams {
    fn visit_statement(&mut self, statement: &ast::Spanned<ast::Statement>) {
        match &statement.node {
            ast::Statement::Let(let_stmt) => {
                self.bound.insert(bare(&let_stmt.name.node).to_string());
            }
            ast::Statement::For(for_stmt) => {
                self.bound.insert(bare(&for_stmt.binding.node).to_string());
            }
            _ => {}
        }
        walk_statement(self, statement);
    }

    fn visit_expr(&mut self, expr: &ast::Spanned<ast::Expr>) {
        match &expr.node {
            ast::Expr::Param(name) => self.used.push((bare(name).to_string(), expr.span)),
            ast::Expr::Closure(closure) => {
                for (name, _) in &closure.params {
                    self.bound.insert(bare(&name.node).to_string());
                }
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

/// A parameter a function body reads is one that something binds (6008).
///
/// **Engine-verified on 3.2.3.** An undeclared name in a body is not a parse
/// error and not a call-time error — it evaluates to `NONE`:
///
/// ```surql
/// DEFINE FUNCTION fn::undeclared() { RETURN $nope; };
/// RETURN fn::undeclared();  -- NONE
/// ```
///
/// So this is a Warning, not an Error: the function runs, and quietly computes
/// the wrong answer. The demo that produced this check had
/// `DEFINE FUNCTION fn::get_permissions($parm: any) { IF $param = "admin" ...`,
/// where `$param = "admin"` is `NONE = "admin"` — always false, so the
/// permission it guards never took its intended branch.
///
/// **What must not fire**, each checked on the engine inside a body:
///
/// * a declared parameter of this function, and a `LET`/`FOR`/closure binding
///   the body makes for itself;
/// * a `DEFINE PARAM` — `DEFINE PARAM $globalp VALUE "hello"` then
///   `DEFINE FUNCTION fn::f() { RETURN $globalp; }` returns `'hello'`. In any
///   source and in any order: a param resolves at call time, so one declared
///   *below* the function still binds it (see [`source_declares_param`]);
/// * an engine-supplied name. `$auth`/`$session`/`$token`/`$access` resolve
///   from the session, and `$this`/`$parent`/`$value`/`$before`/`$after`/
///   `$event`/`$input` are accepted and evaluate to `NONE` — the engine binds
///   them per context, so the analyzer is not entitled to call one unbound.
///   [`crate::context_params`] is the one list of these;
/// * a name bound by an enclosing `LET`. A body reads the *caller's* scope:
///   `LET $outer = "x"; DEFINE FUNCTION fn::f() { RETURN $outer; }` returns
///   `'x'` at the call. `ctx.local` is that scope, so anything in it is
///   exempt — which also covers the session params seeded there.
fn check_body_params_declared(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineFunction) {
    let Some(body) = &stmt.body else {
        return;
    };

    let declared: Vec<String> = stmt
        .params
        .iter()
        .map(|(name, _)| bare(&name.node).to_string())
        .collect();

    let mut walker = BodyParams::default();
    for statement in &body.statements {
        walker.visit_statement(statement);
    }

    for (name, range) in &walker.used {
        if declared.iter().any(|declared| declared == name)
            || walker.bound.contains(name)
            || crate::context_params::is_engine_param(name)
            || ctx.schema().param(name).is_some()
            || ctx.local(name).is_some()
            || source_declares_param(ctx, name)
        {
            continue;
        }

        let span = SourceSpan::new(ctx.source().clone(), *range);
        let mut finding = surrealql_analyzer_diagnostics::catalog::finding(
            span,
            6008,
            format!(
                "`${name}` is not a parameter of `{}`, and nothing else binds it",
                stmt.name.node
            ),
        );
        finding = match crate::suggest::closest(name, declared.iter().map(String::as_str)) {
            Some(nearest) => finding.with_help(format!("did you mean `${nearest}`?")),
            None => finding
                .with_help("an unbound parameter in a function body evaluates to NONE".to_string()),
        };
        let name_span = SourceSpan::new(ctx.source().clone(), stmt.name.span);
        ctx.emit(finding.with_related(
            name_span,
            format!("`{}` declares its parameters here", stmt.name.node),
        ));
    }
}

pub(crate) fn analyze_define_function(
    ctx: &mut AnalysisContext<'_>,
    stmt: &ast::DefineFunction,
) -> Kind {
    check_duplicate_function(ctx, stmt);
    check_body_params_declared(ctx, stmt);
    let Some(body_kind) = infer_function_body_kind(ctx, stmt) else {
        return Kind::None;
    };

    if let Some(return_ty) = &stmt.return_ty {
        if let Some(declared) =
            crate::schema::kind_from_type_expr(&return_ty.node, ctx.source_text()).kind
        {
            // `-> 'a' | 'b' { RETURN 'c'; }` compared the widened `string`,
            // which can never be shown to fall outside a string-literal union.
            // A body that provably returns one constant is checked as that
            // constant. Only the single-`RETURN` shape folds today: a body with
            // several exits is a set of positions rather than one value, and
            // splitting 2012 across them is the compositional-checking work,
            // not this.
            let actual = returned_constant_kind(stmt).unwrap_or_else(|| body_kind.clone());
            let contract = Contract::new(Position::FunctionReturn, declared.clone());
            if contract.decide(&actual).is_violation() {
                let span = surrealql_analyzer_syntax::span::SourceSpan::new(
                    ctx.source().clone(),
                    return_ty.span,
                );
                let name_span = surrealql_analyzer_syntax::span::SourceSpan::new(
                    ctx.source().clone(),
                    stmt.name.span,
                );
                ctx.emit(
                    surrealql_analyzer_diagnostics::catalog::finding(
                        span,
                        contract.code(),
                        format!(
                            "`{}` declares `-> {}` but its body returns `{}`",
                            stmt.name.node,
                            crate::render_kind(&declared),
                            crate::render::render_offending(&actual, Some(&declared))
                        ),
                    )
                    .with_related(name_span, format!("`{}` is defined here", stmt.name.node)),
                );
            }
        }
    }

    Kind::None
}
