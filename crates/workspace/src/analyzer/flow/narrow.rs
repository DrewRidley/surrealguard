//! Control-flow type narrowing.
//!
//! A guard condition partitions a param's value space; each reachable region
//! downstream sees the param narrowed to the kind consistent with the guard.
//! This module recognizes two guard shapes on a param/`LET`-bound `$x`:
//!
//! - **NONE guards** — `$x = NONE`, `$x IS NONE`, `$x != NONE`,
//!   `$x IS NOT NONE`. The positive form narrows `$x` to its non-none kind;
//!   the negative form is the complement.
//! - **Discriminant guards** — `type::table($x) = 'lit'` / `!= 'lit'` on a
//!   `record<...>` union. The positive `=` narrows to `record<lit>`; the
//!   negative removes `lit` from the union.
//!
//! The extracted refinements ([`Effect`]s) are applied to the environment so
//! every downstream read of `$x` (function-argument checks, field access)
//! sees the narrowed kind. This generalizes the single-expression
//! `= NONE OR` / `!= NONE AND` narrowing (`infer::none_guarded_param`) to
//! statement and branch flow.

use surrealdb_types::{Kind, Table};
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::expression::infer::narrow_out_none;
use crate::statement_env::StatementEnv;

/// A refinement a guard proves about a param at a program point.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Narrowing {
    /// The param is NONE.
    None,
    /// The param is not NONE (strip `none`/`null` from its kind).
    NotNone,
    /// The param's record narrows to a single table.
    Table(String),
    /// The param's record excludes a table.
    NotTable(String),
}

/// A `(param, refinement)` a guard yields for one param.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Effect {
    /// The narrowed param's name (without `$`).
    pub param: String,
    /// The refinement to apply to it.
    pub narrowing: Narrowing,
}

/// The refinements that hold in the region where `cond` is TRUE (an `IF`'s
/// THEN body). `env` resolves indirect discriminants (a `LET`-bound
/// `type::table($x)`).
pub(crate) fn positive_effects(cond: &ast::Expr, env: &StatementEnv) -> Vec<Effect> {
    effects(cond, true, env)
}

/// The refinements that hold in the region where `cond` is FALSE (an `IF`'s
/// ELSE body, or the fall-through after a diverging guard).
pub(crate) fn negative_effects(cond: &ast::Expr, env: &StatementEnv) -> Vec<Effect> {
    effects(cond, false, env)
}

/// The refinements a condition proves under the requested polarity. Compound
/// conditions distribute soundly: only the disjunct/conjunct combinations
/// that pin a definite fact contribute — `A OR B` narrows nothing when true
/// (either disjunct might be the reason) but yields `¬A ∧ ¬B` when false, and
/// `A AND B` is the mirror.
fn effects(cond: &ast::Expr, positive: bool, env: &StatementEnv) -> Vec<Effect> {
    match cond {
        ast::Expr::Binary { lhs, op, rhs } => match &op.node {
            ast::BinaryOp::Or => {
                if positive {
                    Vec::new()
                } else {
                    let mut result = effects(&lhs.node, false, env);
                    result.extend(effects(&rhs.node, false, env));
                    result
                }
            }
            ast::BinaryOp::And => {
                if positive {
                    let mut result = effects(&lhs.node, true, env);
                    result.extend(effects(&rhs.node, true, env));
                    result
                } else {
                    Vec::new()
                }
            }
            op => leaf_effect(&lhs.node, op, &rhs.node, positive, env)
                .into_iter()
                .collect(),
        },
        _ => Vec::new(),
    }
}

/// The refinement a single comparison proves under `positive`, if it is a
/// recognized guard shape.
fn leaf_effect(
    lhs: &ast::Expr,
    op: &ast::BinaryOp,
    rhs: &ast::Expr,
    positive: bool,
    env: &StatementEnv,
) -> Option<Effect> {
    // NONE guard: `$x = NONE` / `$x IS NONE` / `$x != NONE` / `$x IS NOT NONE`.
    if let Some(equals_none) = none_test_polarity(op) {
        if let Some(param) = none_guard_param(lhs, rhs) {
            // `$x = NONE` true ⇒ None; `$x != NONE` true ⇒ NotNone.
            let when_true = if equals_none {
                Narrowing::None
            } else {
                Narrowing::NotNone
            };
            let narrowing = if positive { when_true } else { when_true.flip_none() };
            return Some(Effect {
                param: param.to_string(),
                narrowing,
            });
        }
    }
    // Discriminant guard: `type::table($x) = 'lit'` / `!= 'lit'`, either the
    // direct call form or a `LET`-bound `$table = 'lit'` (indirect).
    if matches!(op, ast::BinaryOp::Eq | ast::BinaryOp::NotEq) {
        if let Some((param, table)) = table_discriminant(lhs, rhs, env) {
            let is_eq = matches!(op, ast::BinaryOp::Eq);
            // `= 'lit'` true ⇒ Table(lit); `!= 'lit'` true ⇒ NotTable(lit).
            let when_true = if is_eq {
                Narrowing::Table(table.clone())
            } else {
                Narrowing::NotTable(table.clone())
            };
            let narrowing = if positive {
                when_true
            } else {
                when_true.flip_table(&table)
            };
            return Some(Effect { param, narrowing });
        }
    }
    None
}

impl Narrowing {
    fn flip_none(self) -> Narrowing {
        match self {
            Narrowing::None => Narrowing::NotNone,
            Narrowing::NotNone => Narrowing::None,
            other => other,
        }
    }

    fn flip_table(self, table: &str) -> Narrowing {
        match self {
            Narrowing::Table(_) => Narrowing::NotTable(table.to_string()),
            Narrowing::NotTable(_) => Narrowing::Table(table.to_string()),
            other => other,
        }
    }
}

/// Whether `op` is an equality-family NONE test: `Some(true)` for
/// `=`/`IS` (equals-none), `Some(false)` for `!=`/`IS NOT` (not-equals-none).
fn none_test_polarity(op: &ast::BinaryOp) -> Option<bool> {
    match op {
        ast::BinaryOp::Eq => Some(true),
        ast::BinaryOp::NotEq => Some(false),
        ast::BinaryOp::Other(text) => {
            let words: Vec<&str> = text.split_whitespace().collect();
            match words.as_slice() {
                [a] if a.eq_ignore_ascii_case("IS") => Some(true),
                [a, b] if a.eq_ignore_ascii_case("IS") && b.eq_ignore_ascii_case("NOT") => {
                    Some(false)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The param compared against a `NONE` literal, from either side of the guard.
fn none_guard_param<'a>(lhs: &'a ast::Expr, rhs: &'a ast::Expr) -> Option<&'a str> {
    let is_none = |e: &ast::Expr| matches!(e, ast::Expr::Literal(ast::Literal::None));
    if is_none(rhs) {
        param_name(lhs)
    } else if is_none(lhs) {
        param_name(rhs)
    } else {
        None
    }
}

/// The `(param, table_literal)` of a table discriminant compared to a string
/// literal, from either side. The discriminated side is either a direct
/// `type::table($param)` call or a `LET`-bound `$table` that holds one
/// (resolved through `env`).
fn table_discriminant(
    lhs: &ast::Expr,
    rhs: &ast::Expr,
    env: &StatementEnv,
) -> Option<(String, String)> {
    let of = |disc_side: &ast::Expr, lit_side: &ast::Expr| {
        let param = discriminated_param(disc_side, env)?;
        let table = string_literal(lit_side)?;
        Some((param.to_string(), table))
    };
    of(lhs, rhs).or_else(|| of(rhs, lhs))
}

/// The record param a discriminant expression tests: a `type::table($param)`
/// call directly, or a `$binding` the env records as one.
fn discriminated_param<'a>(expr: &'a ast::Expr, env: &'a StatementEnv) -> Option<&'a str> {
    type_table_param(expr).or_else(|| match expr {
        ast::Expr::Param(binding) => env.table_discriminant(binding),
        _ => None,
    })
}

/// The param of a `type::table($param)` call, borrowed.
fn type_table_param(expr: &ast::Expr) -> Option<&str> {
    let ast::Expr::Call(call) = expr else {
        return None;
    };
    if call.path.node != "type::table" {
        return None;
    }
    let [arg] = call.args.as_slice() else {
        return None;
    };
    param_name(&arg.node)
}

/// The param name, when `expr` is a `type::table($param)` call — used to seed
/// the indirect-discriminant map from `LET $t = type::table($param)`.
pub(crate) fn type_table_arg(expr: &ast::Expr) -> Option<String> {
    type_table_param(expr).map(str::to_string)
}

/// The string a literal holds, if `expr` is a plain string literal.
fn string_literal(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Literal(ast::Literal::String(text)) => Some(text.clone()),
        _ => None,
    }
}

/// The param name, if `expr` is a bare `$name`.
fn param_name(expr: &ast::Expr) -> Option<&str> {
    match expr {
        ast::Expr::Param(name) => Some(name.as_str()),
        _ => None,
    }
}

/// Rebinds each effect's param to its narrowed kind in the current scope.
/// Unbound params (host params, context-only names) and refinements that
/// don't apply to the current kind are skipped, so narrowing only ever
/// tightens a known binding.
pub(crate) fn apply_effects(ctx: &mut AnalysisContext<'_>, effects: &[Effect]) {
    for effect in effects {
        let Some(fact) = ctx.env().let_fact(&effect.param).cloned() else {
            continue;
        };
        let Some(kind) = fact.kind.as_ref() else {
            continue;
        };
        let Some(narrowed) = narrow_kind(kind, &effect.narrowing) else {
            continue;
        };
        let mut new_fact = fact;
        new_fact.kind = Some(narrowed);
        ctx.define_local(effect.param.clone(), new_fact);
    }
}

/// The kind `kind` refines to under `narrowing`, or `None` when the
/// refinement leaves it unchanged (or cannot apply).
fn narrow_kind(kind: &Kind, narrowing: &Narrowing) -> Option<Kind> {
    match narrowing {
        Narrowing::None => Some(Kind::None),
        Narrowing::NotNone => {
            let narrowed = narrow_out_none(kind);
            (narrowed != *kind).then_some(narrowed)
        }
        Narrowing::Table(table) => narrow_record_to(kind, table),
        Narrowing::NotTable(table) => narrow_record_without(kind, table),
    }
}

/// Narrows a `record<...>` (optionally wrapped in a union) to the single
/// `record<table>`, when `table` is among its targets. An unconstrained
/// `record<>` narrows to `record<table>`.
fn narrow_record_to(kind: &Kind, table: &str) -> Option<Kind> {
    match kind {
        Kind::Record(tables) => {
            let present = tables.is_empty() || tables.iter().any(|t| t.to_string() == table);
            present.then(|| Kind::Record(vec![Table::from(table)]))
        }
        Kind::Either(variants) => {
            let narrowed: Vec<Kind> = variants
                .iter()
                .filter_map(|variant| narrow_record_to(variant, table))
                .collect();
            match narrowed.len() {
                0 => None,
                1 => narrowed.into_iter().next(),
                _ => Some(Kind::either(narrowed)),
            }
        }
        _ => None,
    }
}

/// Removes `table` from a `record<...>` union (through an enclosing union),
/// e.g. `record<file | folder>` minus `folder` becomes `record<file>`.
fn narrow_record_without(kind: &Kind, table: &str) -> Option<Kind> {
    match kind {
        Kind::Record(tables) => {
            let kept: Vec<Table> = tables
                .iter()
                .filter(|t| t.to_string() != table)
                .cloned()
                .collect();
            // Nothing removed (an unconstrained `record<>` or absent table)
            // leaves the kind unchanged.
            (!tables.is_empty() && kept.len() != tables.len()).then(|| Kind::Record(kept))
        }
        Kind::Either(variants) => {
            let narrowed: Vec<Kind> = variants
                .iter()
                .map(|variant| narrow_record_without(variant, table).unwrap_or_else(|| variant.clone()))
                .collect();
            Some(Kind::either(narrowed))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_syntax::ast;
    use surrealguard_syntax::lower::lower_first_expr;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    fn cond(query: &str) -> ast::Expr {
        let parsed = parse_source(SourceId::new("narrow:test"), query).expect("parses");
        lower_first_expr(&parsed, "BinaryExpression")
            .expect("has a binary condition")
            .node
    }

    #[test]
    fn none_guards_narrow_both_polarities() {
        let env = StatementEnv::default();
        for (query, pos, neg) in [
            ("RETURN $x = NONE;", Narrowing::None, Narrowing::NotNone),
            ("RETURN $x != NONE;", Narrowing::NotNone, Narrowing::None),
            ("RETURN $x IS NONE;", Narrowing::None, Narrowing::NotNone),
            ("RETURN $x IS NOT NONE;", Narrowing::NotNone, Narrowing::None),
        ] {
            let c = cond(query);
            assert_eq!(
                positive_effects(&c, &env),
                vec![Effect {
                    param: "x".into(),
                    narrowing: pos
                }],
                "positive: {query}"
            );
            assert_eq!(
                negative_effects(&c, &env),
                vec![Effect {
                    param: "x".into(),
                    narrowing: neg
                }],
                "negative: {query}"
            );
        }
    }

    #[test]
    fn type_table_discriminant_narrows_records() {
        let env = StatementEnv::default();
        let c = cond("RETURN type::table($r) = 'folder';");
        assert_eq!(
            positive_effects(&c, &env),
            vec![Effect {
                param: "r".into(),
                narrowing: Narrowing::Table("folder".into())
            }]
        );
        assert_eq!(
            negative_effects(&c, &env),
            vec![Effect {
                param: "r".into(),
                narrowing: Narrowing::NotTable("folder".into())
            }]
        );
    }

    #[test]
    fn indirect_discriminant_resolves_a_let_bound_type_table() {
        // `LET $t = type::table($r); IF $t = 'folder'` narrows `$r` the same
        // as the direct call form.
        let mut env = StatementEnv::default();
        env.set_table_discriminant("t".into(), Some("r".into()));
        let c = cond("RETURN $t = 'folder';");
        assert_eq!(
            positive_effects(&c, &env),
            vec![Effect {
                param: "r".into(),
                narrowing: Narrowing::Table("folder".into())
            }]
        );
        // Without the mapping, `$t = 'folder'` narrows nothing.
        let bare = StatementEnv::default();
        assert!(positive_effects(&c, &bare).is_empty());
    }

    #[test]
    fn or_of_none_guards_yields_both_negatives_on_fall_through() {
        // `$a = NONE OR $b = NONE` proves nothing when true, but its
        // fall-through proves both are non-none.
        let env = StatementEnv::default();
        let c = cond("RETURN $a = NONE OR $b = NONE;");
        assert!(positive_effects(&c, &env).is_empty());
        assert_eq!(
            negative_effects(&c, &env),
            vec![
                Effect {
                    param: "a".into(),
                    narrowing: Narrowing::NotNone
                },
                Effect {
                    param: "b".into(),
                    narrowing: Narrowing::NotNone
                }
            ]
        );
    }

    #[test]
    fn record_union_narrows_to_and_without_a_table() {
        let union = Kind::Record(vec![Table::from("file"), Table::from("folder")]);
        assert_eq!(
            narrow_kind(&union, &Narrowing::Table("folder".into())),
            Some(Kind::Record(vec![Table::from("folder")]))
        );
        assert_eq!(
            narrow_kind(&union, &Narrowing::NotTable("folder".into())),
            Some(Kind::Record(vec![Table::from("file")]))
        );
        // A table not in the union cannot be narrowed to.
        assert_eq!(narrow_kind(&union, &Narrowing::Table("ghost".into())), None);
    }

    #[test]
    fn not_none_strips_the_option() {
        let option = Kind::Either(vec![Kind::None, Kind::String]);
        assert_eq!(
            narrow_kind(&option, &Narrowing::NotNone),
            Some(Kind::String)
        );
        // A non-optional kind is left unchanged (no effect).
        assert_eq!(narrow_kind(&Kind::String, &Narrowing::NotNone), None);
    }

    // --- End-to-end narrowing over the analysis pipeline -------------------

    /// The number of findings with `code` when the source is analyzed as a
    /// workspace (schema + function bodies).
    fn code_count(source: &str, code: u16) -> usize {
        let mut workspace = crate::analysis::Workspace::default();
        workspace.add_virtual_source("narrow-e2e".into(), source.into());
        let output = crate::analysis::analyze_workspace(&workspace);
        output
            .diagnostics
            .iter()
            .filter(|finding| finding.code().number() == code)
            .count()
    }

    const TABLES: &str = "DEFINE TABLE file SCHEMAFULL;\n\
         DEFINE TABLE folder SCHEMAFULL;\n\
         DEFINE TABLE organization SCHEMAFULL;\n\
         DEFINE FUNCTION fn::file_only($f: record<file>) { RETURN true; };\n\
         DEFINE FUNCTION fn::folder_only($f: record<folder>) { RETURN true; };\n\
         DEFINE FUNCTION fn::org_only($o: record<organization>) { RETURN true; };\n";

    #[test]
    fn none_early_return_narrows_the_optional_param() {
        // An early-return NONE guard clears the 5002 an unguarded
        // `option<record<organization>>` would raise at the file-only call.
        let guarded = format!(
            "{TABLES}\
             DEFINE FUNCTION fn::caller($o: option<record<organization>>) {{\n\
                IF $o = NONE THEN RETURN false END;\n\
                RETURN fn::org_only($o);\n\
             }};"
        );
        assert_eq!(code_count(&guarded, 5002), 0, "guarded should be clean");

        // The same call WITHOUT the guard genuinely fails — proving the guard
        // is what clears it.
        let unguarded = format!(
            "{TABLES}\
             DEFINE FUNCTION fn::caller($o: option<record<organization>>) {{\n\
                RETURN fn::org_only($o);\n\
             }};"
        );
        assert_eq!(code_count(&unguarded, 5002), 1, "unguarded must fire");
    }

    #[test]
    fn type_table_discriminant_narrows_the_fall_through_and_then_branch() {
        // Fall-through: after the diverging folder guard, `$r` is `record<file>`.
        let fall_through = format!(
            "{TABLES}\
             DEFINE FUNCTION fn::rec($r: record<file | folder>) {{\n\
                IF type::table($r) = 'folder' THEN RETURN fn::folder_only($r) END;\n\
                RETURN fn::file_only($r);\n\
             }};"
        );
        assert_eq!(code_count(&fall_through, 5002), 0, "narrowed both sides");
    }

    #[test]
    fn branch_merge_widens_back_to_the_union_at_the_join() {
        // Neither branch diverges, so past the join `$r` is the full union
        // again — the file-only call must still fire.
        let widened = format!(
            "{TABLES}\
             DEFINE FUNCTION fn::rec($r: record<file | folder>) {{\n\
                IF type::table($r) = 'folder' {{ LET $a = 1; }} ELSE {{ LET $b = 2; }};\n\
                RETURN fn::file_only($r);\n\
             }};"
        );
        assert_eq!(code_count(&widened, 5002), 1, "union not leaked-narrowed");
    }

    #[test]
    fn indirect_discriminant_narrows_through_a_let_binding() {
        // `LET $t = type::table($r); IF $t = 'folder' THEN ...` narrows `$r`.
        let indirect = format!(
            "{TABLES}\
             DEFINE FUNCTION fn::rec($r: record<file | folder>) {{\n\
                LET $t = type::table($r);\n\
                IF $t = 'folder' THEN RETURN fn::folder_only($r) END;\n\
                RETURN fn::file_only($r);\n\
             }};"
        );
        assert_eq!(code_count(&indirect, 5002), 0);
    }
}
