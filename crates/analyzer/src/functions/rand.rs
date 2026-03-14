use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_args, expect_args_range, expect_min_args};

const KNOWN_FUNCS: &[&str] = &[
    "bool", "enum", "float", "guid", "int", "string", "time", "ulid",
    "uuid", "uuid::v4", "uuid::v7",
];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        "bool" => {
            expect_args("rand::bool", arg_types, 0, node, ctx);
            Kind::Bool
        }
        "enum" => {
            expect_min_args("rand::enum", arg_types, 1, node, ctx);
            Kind::Any
        }
        "float" => {
            expect_args_range("rand::float", arg_types, 0, 2, node, ctx);
            Kind::Float
        }
        "guid" => {
            expect_args_range("rand::guid", arg_types, 0, 1, node, ctx);
            Kind::String
        }
        "int" => {
            expect_args_range("rand::int", arg_types, 0, 2, node, ctx);
            Kind::Int
        }
        "string" => {
            expect_args_range("rand::string", arg_types, 0, 2, node, ctx);
            Kind::String
        }
        "time" => {
            expect_args_range("rand::time", arg_types, 0, 2, node, ctx);
            Kind::Datetime
        }
        "ulid" => {
            expect_args("rand::ulid", arg_types, 0, node, ctx);
            Kind::String
        }
        "uuid" | "uuid::v4" | "uuid::v7" => {
            expect_args(&format!("rand::{}", func), arg_types, 0, node, ctx);
            Kind::Uuid
        }

        _ => {
            emit_unknown_function(
                &format!("rand::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}
