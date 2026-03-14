use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_arg_type, expect_args, is_object};

const KNOWN_FUNCS: &[&str] = &[
    "entries", "from_entries", "keys", "len", "values", "is_empty",
];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        "entries" => {
            expect_args("object::entries", arg_types, 1, node, ctx);
            expect_arg_type("object::entries", arg_types, 0, "object", is_object, node, ctx);
            Kind::Array(Box::new(Kind::Array(Box::new(Kind::Any), None)), None)
        }
        "from_entries" => {
            expect_args("object::from_entries", arg_types, 1, node, ctx);
            Kind::Object
        }
        "keys" => {
            expect_args("object::keys", arg_types, 1, node, ctx);
            expect_arg_type("object::keys", arg_types, 0, "object", is_object, node, ctx);
            Kind::Array(Box::new(Kind::String), None)
        }
        "len" => {
            expect_args("object::len", arg_types, 1, node, ctx);
            expect_arg_type("object::len", arg_types, 0, "object", is_object, node, ctx);
            Kind::Int
        }
        "values" => {
            expect_args("object::values", arg_types, 1, node, ctx);
            expect_arg_type("object::values", arg_types, 0, "object", is_object, node, ctx);
            Kind::Array(Box::new(Kind::Any), None)
        }
        "is_empty" => {
            expect_args("object::is_empty", arg_types, 1, node, ctx);
            expect_arg_type("object::is_empty", arg_types, 0, "object", is_object, node, ctx);
            Kind::Bool
        }

        _ => {
            emit_unknown_function(
                &format!("object::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}
