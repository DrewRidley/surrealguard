use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::expect_args;

const KNOWN_FUNCS: &[&str] = &["ac", "db", "id", "ip", "ns", "origin", "rd", "token"];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        "ac" => {
            expect_args("session::ac", arg_types, 0, node, ctx);
            Kind::String
        }
        "db" => {
            expect_args("session::db", arg_types, 0, node, ctx);
            Kind::String
        }
        "id" => {
            expect_args("session::id", arg_types, 0, node, ctx);
            Kind::String
        }
        "ip" => {
            expect_args("session::ip", arg_types, 0, node, ctx);
            Kind::String
        }
        "ns" => {
            expect_args("session::ns", arg_types, 0, node, ctx);
            Kind::String
        }
        "origin" => {
            expect_args("session::origin", arg_types, 0, node, ctx);
            Kind::String
        }
        "rd" => {
            expect_args("session::rd", arg_types, 0, node, ctx);
            Kind::String
        }
        "token" => {
            expect_args("session::token", arg_types, 0, node, ctx);
            Kind::Object
        }

        _ => {
            super::emit_unknown_function(
                &format!("session::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}
