use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_arg_type, expect_args, is_string};

const KNOWN_FUNCS: &[&str] = &["base64::decode", "base64::encode"];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        "base64::decode" => {
            expect_args("encoding::base64::decode", arg_types, 1, node, ctx);
            expect_arg_type("encoding::base64::decode", arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }
        "base64::encode" => {
            expect_args("encoding::base64::encode", arg_types, 1, node, ctx);
            expect_arg_type("encoding::base64::encode", arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }

        _ => {
            emit_unknown_function(
                &format!("encoding::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}
