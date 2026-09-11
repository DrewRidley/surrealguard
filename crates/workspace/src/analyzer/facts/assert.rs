//! A field's `ASSERT`, folded against a constant `$value`.
//!
//! Two contracts share one question — *does this predicate provably reject
//! this constant?* — and answer it at different moments: 2037 asks it of the
//! field's own `DEFAULT` while the field is being defined, and 2038 asks it of
//! every constant a statement writes into the field afterwards. The fold is
//! the analyzer's one folder ([`super::eval`]) with `$value` bound to the
//! constant, and the answer is `true` only when the predicate folds to a
//! definite `false`.
//!
//! The predicate travels on the schema index as a [`FieldAssert`] so a write
//! site, which holds a `TableDef` and not the `DEFINE FIELD` statement, can
//! still ask.

use surrealql_analyzer_syntax::ast;
use surrealql_analyzer_syntax::span::SourceSpan;

use super::term::{fold_bool, Bindings, ConstValue};

/// A field's `ASSERT` predicate, kept on its [`FieldDef`] so a constant
/// written to the field can be folded against it (2038).
///
/// [`FieldDef`]: crate::schema::FieldDef
#[derive(Clone, Debug, PartialEq)]
pub struct FieldAssert {
    expr: ast::Expr,
    span: SourceSpan,
}

/// `ast::Expr` is only `PartialEq` because a float literal is an `f64`. Every
/// float the parser produces comes from source text, so it is never `NaN`
/// and equality is reflexive over every value this type can hold — which is
/// all `Eq` promises. The schema types derive `Eq`, so this must too.
impl Eq for FieldAssert {}

impl FieldAssert {
    /// The predicate `expr`, declared at `span`.
    pub(crate) fn new(expr: &ast::Expr, span: SourceSpan) -> Self {
        Self {
            expr: expr.clone(),
            span,
        }
    }

    /// Where the `ASSERT` clause is written, for a finding's related note.
    pub fn span(&self) -> &SourceSpan {
        &self.span
    }

    /// Whether the predicate provably rejects `value` — see
    /// [`constant_violates_assert`].
    pub(crate) fn rejects(&self, value: &ConstValue) -> bool {
        constant_violates_assert(&self.expr, value)
    }
}

/// Whether `assert` folds to a definite `false` with `$value` bound to
/// `value`.
///
/// `true` is a proof: every term the predicate's outcome depends on is a
/// constant the folder models — the bound `$value`, literals, comparisons,
/// `AND`/`OR`/`NOT`, membership in an array literal of constants — and the
/// engine would reject the write. Anything else is `false`: a predicate that
/// folds to `true`, and equally one that reaches a term the folder cannot
/// prove (`$this.other`, `string::len($value)`, a subquery). Silence is the
/// only sound answer to uncertainty, because both callers report an error on
/// a `true`.
///
/// `NONE` and `NULL` are never rejected. The engine skips a field's `ASSERT`
/// when the value is `NONE` and the field is optional, and a `NONE`/`NULL`
/// written into a field that does not admit it is a type violation (2001),
/// which is the contract that owns that write.
pub(crate) fn constant_violates_assert(assert: &ast::Expr, value: &ConstValue) -> bool {
    if matches!(value, ConstValue::None | ConstValue::Null) {
        return false;
    }
    fold_bool(assert, Bindings::value(value)) == Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealql_analyzer_syntax::parse::parse_source;
    use surrealql_analyzer_syntax::source::SourceId;

    fn predicate(source: &str) -> ast::Expr {
        let query = format!("RETURN {source};");
        let parsed = parse_source(SourceId::new("assert:test"), query.as_str()).expect("parses");
        let statements = surrealql_analyzer_syntax::lower::lower_statements(&parsed);
        let ast::Statement::Return(stmt) = &statements.first().expect("one statement").node else {
            panic!("expected a RETURN");
        };
        stmt.value.as_ref().expect("a value").node.clone()
    }

    fn rejects(assert: &str, value: ConstValue) -> bool {
        constant_violates_assert(&predicate(assert), &value)
    }

    #[test]
    fn a_constant_outside_the_allowed_set_is_rejected() {
        let allowed = "$value IN ['active', 'inactive']";
        assert!(rejects(allowed, ConstValue::Str("activ".into())));
        assert!(!rejects(allowed, ConstValue::Str("active".into())));
        assert!(rejects("$value > 0 AND $value < 10", ConstValue::Int(12)));
        assert!(!rejects("$value > 0 AND $value < 10", ConstValue::Int(5)));
        assert!(rejects("!($value = 'a')", ConstValue::Str("a".into())));
    }

    #[test]
    fn an_unprovable_term_is_never_a_rejection() {
        // The folder does not model these; a non-match cannot be proven.
        assert!(!rejects(
            "string::len($value) > 3",
            ConstValue::Str("ab".into())
        ));
        assert!(!rejects("$value.len() > 3", ConstValue::Str("ab".into())));
        assert!(!rejects("$value IN $allowed", ConstValue::Str("x".into())));
        assert!(!rejects("$value = $this.other", ConstValue::Int(1)));
        // …but a provably-false side of an AND decides the whole.
        assert!(rejects(
            "$value = 'a' AND $this.ok",
            ConstValue::Str("b".into())
        ));
    }

    #[test]
    fn the_sentinels_are_the_type_contracts_business() {
        assert!(!rejects("$value IN ['a']", ConstValue::None));
        assert!(!rejects("$value IN ['a']", ConstValue::Null));
    }
}
