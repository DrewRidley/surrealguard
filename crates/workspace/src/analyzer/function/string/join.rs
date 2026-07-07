//! `string::join` function analysis: `string::join(separator, ...string) -> string`.
//!
//! Variadic: the first argument is the separator, the remaining arguments are
//! the strings joined by it.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub fn analyze_string_join(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 1,
            max_args: None,
            arg_kinds: vec![ParamKind::Exact(Kind::String)],
            return_kind: ReturnKind::Fixed(Kind::String),
        },
        args,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::function::signature::evaluate;

    fn signature() -> Signature {
        Signature {
            min_args: 1,
            max_args: None,
            arg_kinds: vec![ParamKind::Exact(Kind::String)],
            return_kind: ReturnKind::Fixed(Kind::String),
        }
    }

    #[test]
    fn returns_string_for_variadic_strings() {
        assert_eq!(
            evaluate(&signature(), &[Kind::String, Kind::String, Kind::String]),
            Kind::String
        );
    }
}
