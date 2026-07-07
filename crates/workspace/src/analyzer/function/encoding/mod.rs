//! `encoding` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod base64_decode;
pub mod base64_encode;
pub mod cbor_decode;
pub mod cbor_encode;
pub mod json_decode;
pub mod json_encode;

pub fn analyze_encoding_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "encoding::base64::decode" => {
            base64_decode::analyze_encoding_base64_decode(ctx, call, args)
        }
        "encoding::base64::encode" => {
            base64_encode::analyze_encoding_base64_encode(ctx, call, args)
        }
        "encoding::cbor::decode" => cbor_decode::analyze_encoding_cbor_decode(ctx, call, args),
        "encoding::cbor::encode" => cbor_encode::analyze_encoding_cbor_encode(ctx, call, args),
        "encoding::json::decode" => json_decode::analyze_encoding_json_decode(ctx, call, args),
        "encoding::json::encode" => json_encode::analyze_encoding_json_encode(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
