//! `crypto::argon2::compare` function analysis: `crypto::argon2::compare(string, string) -> bool`.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub fn analyze_crypto_argon2_compare(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![
                ParamKind::Exact(Kind::String),
                ParamKind::Exact(Kind::String),
            ],
            return_kind: ReturnKind::Fixed(Kind::Bool),
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
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![
                ParamKind::Exact(Kind::String),
                ParamKind::Exact(Kind::String),
            ],
            return_kind: ReturnKind::Fixed(Kind::Bool),
        }
    }

    #[test]
    fn compares_hash_and_plaintext_to_bool() {
        assert_eq!(
            evaluate(&signature(), &[Kind::String, Kind::String]),
            Kind::Bool
        );
    }
}
