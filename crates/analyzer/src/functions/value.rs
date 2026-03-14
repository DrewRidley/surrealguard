use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_args};

const KNOWN_FUNCS: &[&str] = &["chain", "diff", "patch"];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        "chain" => {
            expect_args("value::chain", arg_types, 2, node, ctx);
            Kind::Any
        }
        "diff" => {
            expect_args("value::diff", arg_types, 2, node, ctx);
            Kind::Array(Box::new(Kind::Object), None)
        }
        "patch" => {
            expect_args("value::patch", arg_types, 2, node, ctx);
            Kind::Any
        }

        _ => {
            emit_unknown_function(
                &format!("value::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}
