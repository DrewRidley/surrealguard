use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_args, expect_min_args};

const KNOWN_FUNCS: &[&str] = &["analyze", "highlight", "offsets", "score"];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        "analyze" => {
            expect_args("search::analyze", arg_types, 2, node, ctx);
            Kind::Array(Box::new(Kind::String), None)
        }
        "highlight" => {
            expect_min_args("search::highlight", arg_types, 3, node, ctx);
            Kind::String
        }
        "offsets" => {
            expect_args("search::offsets", arg_types, 1, node, ctx);
            Kind::Object
        }
        "score" => {
            expect_args("search::score", arg_types, 1, node, ctx);
            Kind::Float
        }

        _ => {
            emit_unknown_function(
                &format!("search::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}
