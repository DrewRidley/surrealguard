//! `type::is_record` function analysis:
//! `type::is_record(any, option<string>) -> bool`.
//!
//! SurrealDB's `is::record((arg, Optional(table)): (Value, Optional<String>))`
//! accepts an optional second argument — a table name the record must belong
//! to. The analyzer mirrors that optional arity so a two-argument call is not
//! flagged as an arity violation (5002).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(2),
        arg_kinds: vec![ParamKind::Any, ParamKind::Exact(Kind::String)],
        return_kind: ReturnKind::Fixed(Kind::Bool),
    }
}

pub(crate) fn analyze_type_is_record(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(ctx, call, &signature(), args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::function::signature::evaluate;

    #[test]
    fn returns_bool_for_one_or_two_arguments() {
        // Single-argument form: `type::is_record($x)`.
        assert_eq!(
            evaluate(&signature(), &[Kind::Record(vec!["user".into()])]),
            Kind::Bool
        );
        // Two-argument form: `type::is_record($x, "user")` — the optional
        // table name is accepted, not an arity violation.
        assert_eq!(
            evaluate(
                &signature(),
                &[Kind::Record(vec!["user".into()]), Kind::String]
            ),
            Kind::Bool
        );
    }
}
