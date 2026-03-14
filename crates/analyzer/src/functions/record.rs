use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_arg_type, expect_args, is_record};

const KNOWN_FUNCS: &[&str] = &["exists", "id", "table", "tb"];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        "exists" => {
            expect_args("record::exists", arg_types, 1, node, ctx);
            expect_arg_type("record::exists", arg_types, 0, "record", is_record, node, ctx);
            Kind::Bool
        }
        "id" => {
            expect_args("record::id", arg_types, 1, node, ctx);
            expect_arg_type("record::id", arg_types, 0, "record", is_record, node, ctx);
            Kind::Any
        }
        "table" | "tb" => {
            let full_name = format!("record::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "record", is_record, node, ctx);
            Kind::String
        }

        _ => {
            emit_unknown_function(
                &format!("record::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}
