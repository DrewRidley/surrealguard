use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_arg_type, expect_args, is_string};

const KNOWN_FUNCS: &[&str] = &[
    "email::host", "email::user",
    "url::domain", "url::host", "url::path", "url::scheme", "url::fragment",
    "url::query", "url::port",
];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        // --- Email parsing ---
        "email::host" => {
            expect_args("parse::email::host", arg_types, 1, node, ctx);
            expect_arg_type("parse::email::host", arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }
        "email::user" => {
            expect_args("parse::email::user", arg_types, 1, node, ctx);
            expect_arg_type("parse::email::user", arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }

        // --- URL parsing ---
        "url::domain" | "url::host" | "url::path" | "url::scheme" | "url::fragment"
        | "url::query" => {
            let full_name = format!("parse::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }
        "url::port" => {
            expect_args("parse::url::port", arg_types, 1, node, ctx);
            expect_arg_type("parse::url::port", arg_types, 0, "string", is_string, node, ctx);
            Kind::Int
        }

        _ => {
            emit_unknown_function(
                &format!("parse::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}
