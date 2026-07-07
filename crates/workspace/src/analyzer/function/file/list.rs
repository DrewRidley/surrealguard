//! `file::list` function analysis:
//! `file::list(bucket: string, opts: option<object>) -> array<object>`.
//!
//! Lists the entries of a bucket, each described by a metadata object.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_file_list(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 1,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Exact(Kind::String), ParamKind::Object],
            return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::Object), None)),
        },
        args,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature() -> Signature {
        Signature {
            min_args: 1,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Exact(Kind::String), ParamKind::Object],
            return_kind: ReturnKind::Fixed(Kind::Array(Box::new(Kind::Object), None)),
        }
    }

    #[test]
    fn list_of_bucket_is_array_of_object() {
        assert_eq!(
            evaluate(&signature(), &[Kind::String]),
            Kind::Array(Box::new(Kind::Object), None)
        );
    }
}
