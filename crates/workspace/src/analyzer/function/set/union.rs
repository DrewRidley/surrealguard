//! `set::union` function analysis: `set::union(set, set) -> set`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_set_union(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Array, ParamKind::Array],
            return_kind: ReturnKind::SameAsArg(0),
        },
        args,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature() -> Signature {
        Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Array, ParamKind::Array],
            return_kind: ReturnKind::SameAsArg(0),
        }
    }

    #[test]
    fn returns_first_set_kind() {
        let set = Kind::Set(Box::new(Kind::String), None);
        assert_eq!(evaluate(&signature(), &[set.clone(), set.clone()]), set);
    }

    #[test]
    fn mistaken_second_argument_still_infers_from_the_first() {
        assert_eq!(
            evaluate(
                &signature(),
                &[Kind::Set(Box::new(Kind::String), None), Kind::Int]
            ),
            Kind::Set(Box::new(Kind::String), None)
        );
    }
}
