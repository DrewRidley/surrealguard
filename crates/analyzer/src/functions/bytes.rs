use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_args};

const KNOWN_FUNCS: &[&str] = &["len"];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        "len" => {
            expect_args("bytes::len", arg_types, 1, node, ctx);
            Kind::Int
        }

        _ => {
            emit_unknown_function(
                &format!("bytes::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}
