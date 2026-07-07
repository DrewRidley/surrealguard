//! `http::get` function analysis: `http::get(url, ...) -> body`.
//!
//! SurrealDB decodes the response by content type: `application/json`
//! parses to a JSON value, `application/octet-stream` to bytes, `text/*`
//! to a string, and anything else to `NONE` — a bounded union, not `any`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

fn http_body_kind() -> Kind {
    let mut variants = match crate::analyzer::function::json_value_kind() {
        Kind::Either(variants) => variants,
        other => vec![other],
    };
    variants.push(Kind::Bytes);
    variants.push(Kind::None);
    Kind::either(variants)
}

pub fn analyze_http_get(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 1,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Exact(Kind::String), ParamKind::Object],
            return_kind: ReturnKind::Fixed(http_body_kind()),
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
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Exact(Kind::String), ParamKind::Object],
            return_kind: ReturnKind::Fixed(http_body_kind()),
        }
    }

    #[test]
    fn returns_the_response_body_union() {
        let kind = evaluate(&signature(), &[Kind::String]);
        let Kind::Either(variants) = kind else {
            panic!("expected a body union, got {kind:?}");
        };
        assert!(variants.contains(&Kind::Object));
        assert!(variants.contains(&Kind::String));
        assert!(variants.contains(&Kind::Bytes));
        assert!(variants.contains(&Kind::None));
    }
}
