use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_args};

const KNOWN_FUNCS: &[&str] = &["id", "tb"];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        "id" => {
            expect_args("meta::id", arg_types, 1, node, ctx);
            Kind::Any // record ID value part (string or int)
        }
        "tb" => {
            expect_args("meta::tb", arg_types, 1, node, ctx);
            Kind::String
        }

        _ => {
            emit_unknown_function(
                &format!("meta::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}
