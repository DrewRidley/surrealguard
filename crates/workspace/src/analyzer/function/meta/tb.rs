//! `meta::tb` function analysis: `meta::tb(record) -> string`.
//!
//! Returns the table name of a record id as a string. Mirrors `record::tb`
//! (same underlying function under a different namespace alias).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub fn analyze_meta_tb(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Any],
            return_kind: ReturnKind::Fixed(Kind::String),
        },
        args,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::function::signature::evaluate;
    use crate::analyzer::function::signature::{ParamKind, ReturnKind, Signature};

    fn signature() -> Signature {
        Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Any],
            return_kind: ReturnKind::Fixed(Kind::String),
        }
    }

    #[test]
    fn returns_string_for_record_argument() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Record(Vec::new())]),
            Kind::String
        );
    }
}
