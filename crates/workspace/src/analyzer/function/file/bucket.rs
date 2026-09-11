//! `file::bucket` function analysis: `file::bucket(file) -> string`.

use surrealdb_types::Kind;
use surrealql_analyzer_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Any],
        return_kind: ReturnKind::Fixed(Kind::String),
    }
}

pub(crate) fn analyze_file_bucket(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    // The file-pointer argument is left `Any`: `Kind::File` compares by exact
    // bucket list, so an inferred `File(["bucket"])` would never match a
    // `File([])` param and would wrongly downgrade the return to `Any`.
    evaluate(&signature(), args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_of_file_is_string() {
        assert_eq!(
            evaluate(&signature(), &[Kind::File(Vec::new())]),
            Kind::String
        );
    }
}
