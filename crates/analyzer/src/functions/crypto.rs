use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_arg_type, expect_args, is_string};

const KNOWN_FUNCS: &[&str] = &[
    "blake3", "md5", "sha1", "sha256", "sha512",
    "argon2::generate", "bcrypt::generate", "pbkdf2::generate", "scrypt::generate",
    "argon2::compare", "bcrypt::compare", "pbkdf2::compare", "scrypt::compare",
];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        // Hash functions: string → string
        "blake3" | "md5" | "sha1" | "sha256" | "sha512" => {
            let full_name = format!("crypto::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }

        // Password hashing: generate(string) → string
        "argon2::generate" | "bcrypt::generate" | "pbkdf2::generate" | "scrypt::generate" => {
            let full_name = format!("crypto::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }

        // Password verify: compare(string, string) → bool
        "argon2::compare" | "bcrypt::compare" | "pbkdf2::compare" | "scrypt::compare" => {
            let full_name = format!("crypto::{}", func);
            expect_args(&full_name, arg_types, 2, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "string", is_string, node, ctx);
            expect_arg_type(&full_name, arg_types, 1, "string", is_string, node, ctx);
            Kind::Bool
        }

        _ => {
            emit_unknown_function(
                &format!("crypto::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}
