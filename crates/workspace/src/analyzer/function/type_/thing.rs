//! `type::thing` function analysis: `type::thing(table, id) -> record`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};

pub fn analyze_type_thing(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = (ctx, call);
    evaluate(
        &Signature {
            min_args: 1,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Any],
            return_kind: ReturnKind::Fixed(Kind::Record(Vec::new())),
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
            arg_kinds: vec![ParamKind::Any],
            return_kind: ReturnKind::Fixed(Kind::Record(Vec::new())),
        }
    }

    #[test]
    fn constructs_unconstrained_record() {
        assert_eq!(
            evaluate(&signature(), &[Kind::String, Kind::Int]),
            Kind::Record(Vec::new())
        );
    }
}
