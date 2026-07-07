//! `encoding::cbor::decode` function analysis:
//! `encoding::cbor::decode(bytes) -> any`.
//!
//! CBOR tags decode to arbitrary SurrealDB values (datetimes, records,
//! durations, ...), so the result is genuinely unconstrained.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub fn analyze_encoding_cbor_decode(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Exact(Kind::Bytes)],
            return_kind: ReturnKind::Fixed(Kind::Any),
        },
        args,
    )
}
