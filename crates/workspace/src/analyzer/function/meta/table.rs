//! `meta::table` function analysis: `meta::table(record) -> table`.
//!
//! Returns the table portion of a record id. Mirrors `record::table` (same
//! underlying function under a different namespace alias).

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Any],
        return_kind: ReturnKind::Fixed(Kind::Table(Vec::new())),
    }
}

pub(crate) fn analyze_meta_table(
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
    fn returns_table_for_record_argument() {
        assert_eq!(
            evaluate(&signature(), &[Kind::Record(Vec::new())]),
            Kind::Table(Vec::new())
        );
    }
}
