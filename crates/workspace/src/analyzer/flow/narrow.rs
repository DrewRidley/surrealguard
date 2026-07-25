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

/// The target of a narrowing: a bare param (`$x`) or a param plus a short
/// field path (`$file.folder`, `$a.b.c`). Bare params rebind their binding;
/// field paths record a per-path override keyed on the exact `param.field…`
/// string — only that path narrows, never the base param or a sibling.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GuardPath {
    /// The base param name (without `$`).
    pub param: String,
    /// The field segments after the param; empty means a bare param.
    pub fields: Vec<String>,
}

impl GuardPath {
    /// A bare-param target.
    pub(crate) fn bare(param: String) -> Self {
        Self {
            param,
            fields: Vec::new(),
        }
    }

    /// Whether this targets the bare param (no field path).
    pub(crate) fn is_bare(&self) -> bool {
        self.fields.is_empty()
    }

    /// The `param.field.field` lookup key (just `param` when bare).
    pub(crate) fn key(&self) -> String {
        if self.fields.is_empty() {
            self.param.clone()
        } else {
            format!("{}.{}", self.param, self.fields.join("."))
        }
    }
}

/// A `(path, refinement)` a guard yields for one param or field path.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Effect {
    /// The narrowed target (bare param or field path).
    pub path: GuardPath,
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
        // A bare boolean guard call: `type::is_record($x)` /
        // `type::is_record($x, 'tbl')`. Positive-only (see `is_record_effect`).
        ast::Expr::Call(call) => is_record_effect(call, positive).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// The refinement a `type::is_record` guard proves. This mirrors the
/// `type::table(...) = 'lit'` discriminant, but the predicate is the *call
/// itself* used as a boolean (not a comparison), and it narrows on the
/// **positive branch only**: `type::is_record($x)` is FALSE when `$x` is NONE,
/// a non-record scalar, or (with a table arg) a record of another table, so a
/// false result proves nothing about the kind.
///
/// - `type::is_record($x)`        → `NotNone` (strip the `none`, leaving the
///   non-none open `record`).
/// - `type::is_record($x, 'tbl')` → `Table("tbl")` (narrow the record union to
///   `record<tbl>`, also dropping the `none`).
///
/// Lowering normalizes `type::is::record` → `type::is_record`
/// (`function/mod.rs:55`), so only the underscore spelling is matched here.
fn is_record_effect(call: &ast::Call, positive: bool) -> Option<Effect> {
    if !positive || call.path.node != "type::is_record" {
        return None;
    }
    let path = guard_path_of(&call.args.first()?.node)?;
    let narrowing = match call.args.get(1) {
        Some(table_arg) => Narrowing::Table(string_literal(&table_arg.node)?),
        None => Narrowing::NotNone,
    };
    Some(Effect { path, narrowing })
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
    // NONE guard: `$x = NONE` / `$x IS NONE` / `$x != NONE` / `$x IS NOT NONE`,
    // on a bare param or a simple field path (`$file.folder != NONE`).
    if let Some(equals_none) = none_test_polarity(op) {
        if let Some(path) = none_guard_path(lhs, rhs) {
            // `$x = NONE` true ⇒ None; `$x != NONE` true ⇒ NotNone.
            let when_true = if equals_none {
                Narrowing::None
            } else {
                Narrowing::NotNone
            };
            let narrowing = if positive { when_true } else { when_true.flip_none() };
            return Some(Effect { path, narrowing });
        }
    }
    // Discriminant guard: `type::table($x) = 'lit'` / `!= 'lit'`, either the
    // direct call form or a `LET`-bound `$table = 'lit'` (indirect).
    if matches!(op, ast::BinaryOp::Eq | ast::BinaryOp::NotEq) {
        if let Some((path, table)) = table_discriminant(lhs, rhs, env) {
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
            return Some(Effect { path, narrowing });
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

/// The param/path compared against a `NONE` literal, from either side.
fn none_guard_path(lhs: &ast::Expr, rhs: &ast::Expr) -> Option<GuardPath> {
    let is_none = |e: &ast::Expr| matches!(e, ast::Expr::Literal(ast::Literal::None));
    if is_none(rhs) {
        guard_path_of(lhs)
    } else if is_none(lhs) {
        guard_path_of(rhs)
    } else {
        None
    }
}

/// The narrowable target an expression names: a bare `$param`, or a simple
/// idiom path `$param.field.field` (plain field parts only). Shared by both
/// the statement/branch guards here and the in-expression `!= NONE AND` /
/// `= NONE OR` narrowing (`infer`).
pub(crate) fn guard_path_of(expr: &ast::Expr) -> Option<GuardPath> {
    match expr {
        ast::Expr::Param(name) => Some(GuardPath::bare(name.clone())),
        ast::Expr::Idiom(idiom) => idiom_guard_path(idiom),
        _ => None,
    }
}

/// The `$param.field.field` path of a simple idiom (a param start followed by
/// one or more plain field segments), if it is one.
fn idiom_guard_path(idiom: &ast::Idiom) -> Option<GuardPath> {
    let mut parts = idiom.parts.iter();
    let ast::IdiomPart::Start(start) = &parts.next()?.node else {
        return None;
    };
    let ast::Expr::Param(param) = &start.node else {
        return None;
    };
    let mut fields = Vec::new();
    for part in parts {
        let ast::IdiomPart::Field(name) = &part.node else {
            return None;
        };
        fields.push(name.clone());
    }
    (!fields.is_empty()).then(|| GuardPath {
        param: param.clone(),
        fields,
    })
}

/// The `(path, table_literal)` of a table discriminant compared to a string
/// literal, from either side. The discriminated side is either a direct
/// `type::table($path)` call or a `LET`-bound `$table` that holds one
/// (resolved through `env`).
fn table_discriminant(
    lhs: &ast::Expr,
    rhs: &ast::Expr,
    env: &StatementEnv,
) -> Option<(GuardPath, String)> {
    let of = |disc_side: &ast::Expr, lit_side: &ast::Expr| {
        let path = discriminated_path(disc_side, env)?;
        let table = string_literal(lit_side)?;
        Some((path, table))
    };
    of(lhs, rhs).or_else(|| of(rhs, lhs))
}

/// The record param/path a discriminant expression tests: a `type::table(...)`
/// call directly, or a `$binding` the env records as one.
fn discriminated_path(expr: &ast::Expr, env: &StatementEnv) -> Option<GuardPath> {
    type_table_path(expr).or_else(|| match expr {
        ast::Expr::Param(binding) => {
            env.table_discriminant(binding).map(|p| GuardPath::bare(p.to_string()))
        }
        _ => None,
    })
}

/// The param/path argument of a `type::table(...)` call.
fn type_table_path(expr: &ast::Expr) -> Option<GuardPath> {
    let ast::Expr::Call(call) = expr else {
        return None;
    };
    if call.path.node != "type::table" {
        return None;
    }
    let [arg] = call.args.as_slice() else {
        return None;
    };
    guard_path_of(&arg.node)
}

/// The param name, when `expr` is a bare `type::table($param)` call — used to
/// seed the indirect-discriminant map from `LET $t = type::table($param)`.
/// (Only bare params are recorded as indirect discriminants.)
pub(crate) fn type_table_arg(expr: &ast::Expr) -> Option<String> {
    match type_table_path(expr) {
        Some(path) if path.is_bare() => Some(path.param),
        _ => None,
    }
}

/// The string a literal holds, if `expr` is a plain string literal.
fn string_literal(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Literal(ast::Literal::String(text)) => Some(text.clone()),
        _ => None,
    }
}

/// Applies each effect's refinement to the current scope. A bare param
/// rebinds its binding to the narrowed kind; a field path records a per-path
/// override keyed on the exact `param.field…` string. Unbound bases and
/// refinements that don't tighten the current kind are skipped, so narrowing
/// only ever tightens a known kind — and never touches the base param or a
/// sibling path.
pub(crate) fn apply_effects(ctx: &mut AnalysisContext<'_>, effects: &[Effect]) {
    for effect in effects {
        let Some(base) = ctx.env().let_fact(&effect.path.param).cloned() else {
            continue;
        };
        let Some(base_kind) = base.kind.clone() else {
            continue;
        };
        if effect.path.is_bare() {
            let Some(narrowed) = narrow_kind(&base_kind, &effect.narrowing) else {
                continue;
            };
            let mut new_fact = base;
            new_fact.kind = Some(narrowed);
            ctx.define_local(effect.path.param.clone(), new_fact);
        } else {
            // Resolve the path's declared kind through the schema, then narrow
            // and record it under the exact path key.
            let Some(current) = crate::analyzer::expression::infer::step_field_path(
                &base_kind,
                &effect.path.fields,
                ctx.schema(),
            ) else {
                continue;
            };
            let Some(narrowed) = narrow_kind(&current, &effect.narrowing) else {
                continue;
            };
            ctx.define_narrowed_path(effect.path.key(), narrowed);
        }
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

    fn call_cond(query: &str) -> ast::Expr {
        let parsed = parse_source(SourceId::new("narrow:test"), query).expect("parses");
        lower_first_expr(&parsed, "FunctionCall")
            .expect("has a call condition")
            .node
    }

    #[test]
    fn type_is_record_with_table_narrows_positive_branch_only() {
        // `type::is_record($auth, 'user')` narrows `$auth` to `record<user>` in
        // the THEN branch; the ELSE branch narrows nothing — false is satisfied
        // by NONE, a non-record scalar, or another table.
        let env = StatementEnv::default();
        let c = call_cond("RETURN type::is_record($auth, 'user');");
        assert_eq!(
            positive_effects(&c, &env),
            vec![Effect {
                path: GuardPath::bare("auth".into()),
                narrowing: Narrowing::Table("user".into()),
            }]
        );
        assert!(
            negative_effects(&c, &env).is_empty(),
            "the negative branch of type::is_record narrows nothing"
        );
    }

    #[test]
    fn bare_type_is_record_strips_none_on_positive_branch_only() {
        // `type::is_record($auth)` (no table arg) strips the NONE, leaving the
        // non-none open `record`; the negative branch narrows nothing.
        let env = StatementEnv::default();
        let c = call_cond("RETURN type::is_record($auth);");
        assert_eq!(
            positive_effects(&c, &env),
            vec![Effect {
                path: GuardPath::bare("auth".into()),
                narrowing: Narrowing::NotNone,
            }]
        );
        assert!(negative_effects(&c, &env).is_empty());
    }

    #[test]
    fn non_is_record_call_narrows_nothing() {
        // A different call is not a recognized guard.
        let env = StatementEnv::default();
        let c = call_cond("RETURN type::is_string($auth);");
        assert!(positive_effects(&c, &env).is_empty());
        assert!(negative_effects(&c, &env).is_empty());
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
                    path: GuardPath::bare("x".into()),
                    narrowing: pos
                }],
                "positive: {query}"
            );
            assert_eq!(
                negative_effects(&c, &env),
                vec![Effect {
                    path: GuardPath::bare("x".into()),
                    narrowing: neg
                }],
                "negative: {query}"
            );
        }
    }

    #[test]
    fn none_guard_on_a_field_path_narrows_that_exact_path() {
        // `$file.folder != NONE` narrows the path `file.folder`, not `$file`.
        let env = StatementEnv::default();
        let path = GuardPath {
            param: "file".into(),
            fields: vec!["folder".into()],
        };
        let c = cond("RETURN $file.folder != NONE;");
        assert_eq!(
            positive_effects(&c, &env),
            vec![Effect {
                path: path.clone(),
                narrowing: Narrowing::NotNone
            }]
        );
        assert_eq!(
            negative_effects(&c, &env),
            vec![Effect {
                path,
                narrowing: Narrowing::None
            }]
        );
    }

    #[test]
    fn type_table_discriminant_narrows_records() {
        let env = StatementEnv::default();
        let c = cond("RETURN type::table($r) = 'folder';");
        assert_eq!(
            positive_effects(&c, &env),
            vec![Effect {
                path: GuardPath::bare("r".into()),
                narrowing: Narrowing::Table("folder".into())
            }]
        );
        assert_eq!(
            negative_effects(&c, &env),
            vec![Effect {
                path: GuardPath::bare("r".into()),
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
                path: GuardPath::bare("r".into()),
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
                    path: GuardPath::bare("a".into()),
                    narrowing: Narrowing::NotNone
                },
                Effect {
                    path: GuardPath::bare("b".into()),
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

    // --- Field-path narrowing (`$file.folder != NONE`) ---------------------

    /// `file` has an optional `folder` link and a sibling optional link; the
    /// callee wants the non-optional `record<folder>`.
    const FIELD_PATH_TABLES: &str = "DEFINE TABLE folder SCHEMAFULL;\n\
         DEFINE TABLE file SCHEMAFULL;\n\
         DEFINE FIELD folder ON file TYPE option<record<folder>>;\n\
         DEFINE FIELD sibling ON file TYPE option<record<folder>>;\n\
         DEFINE FUNCTION fn::folder_only($f: record<folder>) { RETURN true; };\n";

    #[test]
    fn field_path_none_guard_narrows_the_and_right_operand() {
        // `$file.folder != NONE AND fn::folder_only($file.folder)` — the right
        // conjunct only runs when the path is non-none, so it type-checks.
        let guarded = format!(
            "{FIELD_PATH_TABLES}\
             DEFINE FUNCTION fn::caller($file: record<file>) {{\n\
                IF $file.folder != NONE AND fn::folder_only($file.folder) THEN RETURN true END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(code_count(&guarded, 5002), 0, "AND-right sees the narrowed path");

        // The same call WITHOUT the guard genuinely fails against the
        // `option<record<folder>>` argument — proving the guard is what clears it.
        let unguarded = format!(
            "{FIELD_PATH_TABLES}\
             DEFINE FUNCTION fn::caller($file: record<file>) {{\n\
                RETURN fn::folder_only($file.folder);\n\
             }};"
        );
        assert_eq!(code_count(&unguarded, 5002), 1, "unguarded must fire");
    }

    #[test]
    fn field_path_none_guard_narrows_the_then_branch() {
        // `IF $file.folder != NONE THEN ... END` narrows the path in the body.
        let guarded = format!(
            "{FIELD_PATH_TABLES}\
             DEFINE FUNCTION fn::caller($file: record<file>) {{\n\
                IF $file.folder != NONE THEN RETURN fn::folder_only($file.folder) END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(code_count(&guarded, 5002), 0, "THEN body sees the narrowed path");
    }

    #[test]
    fn field_path_guard_does_not_narrow_a_sibling_path() {
        // The must-not-narrow boundary: guarding `$file.folder` proves nothing
        // about `$file.sibling`, which is still `option<record<folder>>`.
        let sibling = format!(
            "{FIELD_PATH_TABLES}\
             DEFINE FUNCTION fn::caller($file: record<file>) {{\n\
                IF $file.folder != NONE AND fn::folder_only($file.sibling) THEN RETURN true END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(
            code_count(&sibling, 5002),
            1,
            "the sibling path must not be narrowed"
        );
    }

    // --- `$auth` seed + narrowing (design §4 Phase 1) ----------------------

    /// `$auth` seeds as `option<record>` in every env, so a callee wanting a
    /// concrete/non-optional record needs a guard to clear the mismatch.
    const AUTH_TABLES: &str = "DEFINE TABLE user SCHEMAFULL;\n\
         DEFINE FUNCTION fn::user_only($u: record<user>) { RETURN true; };\n\
         DEFINE FUNCTION fn::any_record($r: record) { RETURN true; };\n";

    #[test]
    fn auth_seeds_as_option_record_so_an_unguarded_record_call_fires() {
        // Baseline: `$auth : option<record>` is not assignable to `record` —
        // proving the seed carries the NONE and reaches a `fn::` body.
        let unguarded = format!(
            "{AUTH_TABLES}\
             DEFINE FUNCTION fn::caller() {{\n\
                RETURN fn::any_record($auth);\n\
             }};"
        );
        assert_eq!(code_count(&unguarded, 5002), 1, "option<record> is not record");
    }

    #[test]
    fn auth_none_guard_narrows_to_record_in_the_then_branch() {
        // `IF $auth != NONE THEN ...` strips the NONE (existing narrow path),
        // leaving the open `record`, so the `fn::any_record($auth)` call clears.
        let guarded = format!(
            "{AUTH_TABLES}\
             DEFINE FUNCTION fn::caller() {{\n\
                IF $auth != NONE THEN RETURN fn::any_record($auth) END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(code_count(&guarded, 5002), 0, "!= NONE narrows $auth to record");
    }

    #[test]
    fn auth_type_is_record_with_table_narrows_to_that_table() {
        // `type::is_record($auth, 'user')` narrows `$auth` to `record<user>` in
        // the THEN branch, so the `fn::user_only($auth)` call type-checks.
        let guarded = format!(
            "{AUTH_TABLES}\
             DEFINE FUNCTION fn::caller() {{\n\
                IF type::is_record($auth, 'user') THEN RETURN fn::user_only($auth) END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(code_count(&guarded, 5002), 0, "positive branch narrows to record<user>");

        // The same call without the guard genuinely fails — the guard is what
        // clears it (also proves the double-colon spelling normalizes).
        let colon = format!(
            "{AUTH_TABLES}\
             DEFINE FUNCTION fn::caller() {{\n\
                IF type::is::record($auth, 'user') THEN RETURN fn::user_only($auth) END;\n\
                RETURN false;\n\
             }};"
        );
        assert_eq!(code_count(&colon, 5002), 0, "type::is::record normalizes to type::is_record");
    }
}
