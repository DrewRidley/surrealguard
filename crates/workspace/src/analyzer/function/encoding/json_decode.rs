//! `encoding::json::decode` function analysis:
//! `encoding::json::decode(string) -> object | array | string | number | bool | null`.
//!
//! SurrealDB parses with `json_to_value`, which only produces JSON-value
//! kinds — a bounded union, not `any`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_encoding_json_decode(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Exact(Kind::String)],
            return_kind: ReturnKind::Fixed(crate::analyzer::function::json_value_kind()),
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
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Exact(Kind::String)],
            return_kind: ReturnKind::Fixed(crate::analyzer::function::json_value_kind()),
        }
    }

    #[test]
    fn returns_the_json_value_union() {
        let kind = evaluate(&signature(), &[Kind::String]);
        let Kind::Either(variants) = kind else {
            panic!("expected a JSON value union, got {kind:?}");
        };
        assert!(variants.contains(&Kind::Object));
        assert!(variants.contains(&Kind::Number));
        assert!(variants.contains(&Kind::Null));
        assert!(!variants.contains(&Kind::Bytes));
    }
}
