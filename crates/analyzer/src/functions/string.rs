use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_arg_type, expect_args, expect_args_range, expect_min_args, is_string};

/// Known string:: function names for fuzzy matching.
const KNOWN_FUNCS: &[&str] = &[
    "concat", "join", "lowercase", "uppercase", "trim", "slug", "reverse",
    "replace", "repeat", "slice", "html::encode", "html::sanitize",
    "contains", "starts_with", "ends_with", "matches", "len", "split", "words",
    "is::alpha", "is::alphanum", "is::ascii", "is::datetime", "is::domain",
    "is::email", "is::hexadecimal", "is::ip", "is::ipv4", "is::ipv6",
    "is::latitude", "is::longitude", "is::numeric", "is::record", "is::semver",
    "is::ulid", "is::url", "is::uuid",
    "distance::damerau_levenshtein", "distance::hamming", "distance::levenshtein",
    "distance::osa_distance", "distance::normalized_damerau_levenshtein",
    "distance::normalized_levenshtein",
    "similarity::fuzzy", "similarity::jaro", "similarity::jaro_winkler",
    "similarity::smithwaterman", "similarity::sorensen_dice",
    "semver::compare", "semver::major", "semver::minor", "semver::patch",
    "semver::inc::major", "semver::inc::minor", "semver::inc::patch",
    "semver::set::major", "semver::set::minor", "semver::set::patch",
];

/// Convert a camelCase function name to snake_case.
/// e.g. "startsWith" -> "starts_with", "toLowerCase" -> "to_lower_case"
fn camel_to_snake(name: &str) -> String {
    let mut result = String::with_capacity(name.len() + 4);
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(ch.to_lowercase().next().unwrap());
    }
    result
}

/// Map certain camelCase snake_case conversions to their actual SurrealDB function names,
/// since the automatic conversion doesn't always match (e.g. "toLowerCase" -> "to_lower_case"
/// but the real function is "lowercase").
fn camel_alias(name: &str) -> Option<&'static str> {
    match name {
        "startsWith" => Some("starts_with"),
        "endsWith" => Some("ends_with"),
        "toLowerCase" => Some("lowercase"),
        "toUpperCase" => Some("uppercase"),
        "trimStart" => Some("trim_start"),
        "trimEnd" => Some("trim_end"),
        _ => None,
    }
}

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        // --- String → String transforms ---
        "concat" => {
            expect_min_args("string::concat", arg_types, 1, node, ctx);
            for i in 0..arg_types.len() {
                expect_arg_type("string::concat", arg_types, i, "string", is_string, node, ctx);
            }
            Kind::String
        }
        "join" => {
            expect_min_args("string::join", arg_types, 2, node, ctx);
            for i in 0..arg_types.len() {
                expect_arg_type("string::join", arg_types, i, "string", is_string, node, ctx);
            }
            Kind::String
        }
        "lowercase" => {
            expect_args("string::lowercase", arg_types, 1, node, ctx);
            expect_arg_type("string::lowercase", arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }
        "uppercase" => {
            expect_args("string::uppercase", arg_types, 1, node, ctx);
            expect_arg_type("string::uppercase", arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }
        "trim" => {
            expect_args("string::trim", arg_types, 1, node, ctx);
            expect_arg_type("string::trim", arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }
        "slug" => {
            expect_args_range("string::slug", arg_types, 1, 2, node, ctx);
            expect_arg_type("string::slug", arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }
        "reverse" => {
            expect_args("string::reverse", arg_types, 1, node, ctx);
            expect_arg_type("string::reverse", arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }
        "replace" => {
            expect_args("string::replace", arg_types, 3, node, ctx);
            expect_arg_type("string::replace", arg_types, 0, "string", is_string, node, ctx);
            expect_arg_type("string::replace", arg_types, 1, "string", is_string, node, ctx);
            expect_arg_type("string::replace", arg_types, 2, "string", is_string, node, ctx);
            Kind::String
        }
        "repeat" => {
            expect_args("string::repeat", arg_types, 2, node, ctx);
            expect_arg_type("string::repeat", arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }
        "slice" => {
            expect_args_range("string::slice", arg_types, 2, 3, node, ctx);
            expect_arg_type("string::slice", arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }

        // --- HTML ---
        "html::encode" => {
            expect_args("string::html::encode", arg_types, 1, node, ctx);
            expect_arg_type("string::html::encode", arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }
        "html::sanitize" => {
            expect_args("string::html::sanitize", arg_types, 1, node, ctx);
            expect_arg_type("string::html::sanitize", arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }

        // --- String → Bool checks ---
        "contains" => {
            expect_args("string::contains", arg_types, 2, node, ctx);
            expect_arg_type("string::contains", arg_types, 0, "string", is_string, node, ctx);
            expect_arg_type("string::contains", arg_types, 1, "string", is_string, node, ctx);
            Kind::Bool
        }
        "starts_with" => {
            expect_args("string::starts_with", arg_types, 2, node, ctx);
            expect_arg_type("string::starts_with", arg_types, 0, "string", is_string, node, ctx);
            expect_arg_type("string::starts_with", arg_types, 1, "string", is_string, node, ctx);
            Kind::Bool
        }
        "ends_with" => {
            expect_args("string::ends_with", arg_types, 2, node, ctx);
            expect_arg_type("string::ends_with", arg_types, 0, "string", is_string, node, ctx);
            expect_arg_type("string::ends_with", arg_types, 1, "string", is_string, node, ctx);
            Kind::Bool
        }
        "matches" => {
            expect_args("string::matches", arg_types, 2, node, ctx);
            expect_arg_type("string::matches", arg_types, 0, "string", is_string, node, ctx);
            expect_arg_type("string::matches", arg_types, 1, "string", is_string, node, ctx);
            Kind::Bool
        }

        // --- is:: checks (all 1 arg → bool) ---
        "is::alpha" | "is::alphanum" | "is::ascii" | "is::datetime" | "is::domain"
        | "is::email" | "is::hexadecimal" | "is::ip" | "is::ipv4" | "is::ipv6"
        | "is::latitude" | "is::longitude" | "is::numeric" | "is::record" | "is::semver"
        | "is::ulid" | "is::url" | "is::uuid" => {
            let full_name = format!("string::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "string", is_string, node, ctx);
            Kind::Bool
        }

        // --- String → Int ---
        "len" => {
            expect_args("string::len", arg_types, 1, node, ctx);
            expect_arg_type("string::len", arg_types, 0, "string", is_string, node, ctx);
            Kind::Int
        }

        // --- Distance functions ---
        "distance::damerau_levenshtein" | "distance::hamming" | "distance::levenshtein"
        | "distance::osa_distance" => {
            let full_name = format!("string::{}", func);
            expect_args(&full_name, arg_types, 2, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "string", is_string, node, ctx);
            expect_arg_type(&full_name, arg_types, 1, "string", is_string, node, ctx);
            Kind::Int
        }
        "distance::normalized_damerau_levenshtein" | "distance::normalized_levenshtein" => {
            let full_name = format!("string::{}", func);
            expect_args(&full_name, arg_types, 2, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "string", is_string, node, ctx);
            expect_arg_type(&full_name, arg_types, 1, "string", is_string, node, ctx);
            Kind::Float
        }

        // --- Similarity functions ---
        "similarity::fuzzy" | "similarity::jaro" | "similarity::jaro_winkler"
        | "similarity::smithwaterman" | "similarity::sorensen_dice" => {
            let full_name = format!("string::{}", func);
            expect_args(&full_name, arg_types, 2, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "string", is_string, node, ctx);
            expect_arg_type(&full_name, arg_types, 1, "string", is_string, node, ctx);
            Kind::Float
        }

        // --- Split / words → array<string> ---
        "split" => {
            expect_args("string::split", arg_types, 2, node, ctx);
            expect_arg_type("string::split", arg_types, 0, "string", is_string, node, ctx);
            expect_arg_type("string::split", arg_types, 1, "string", is_string, node, ctx);
            Kind::Array(Box::new(Kind::String), None)
        }
        "words" => {
            expect_args("string::words", arg_types, 1, node, ctx);
            expect_arg_type("string::words", arg_types, 0, "string", is_string, node, ctx);
            Kind::Array(Box::new(Kind::String), None)
        }

        // --- Semver ---
        "semver::compare" => {
            expect_args("string::semver::compare", arg_types, 2, node, ctx);
            expect_arg_type("string::semver::compare", arg_types, 0, "string", is_string, node, ctx);
            expect_arg_type("string::semver::compare", arg_types, 1, "string", is_string, node, ctx);
            Kind::Int
        }
        "semver::major" | "semver::minor" | "semver::patch" => {
            let full_name = format!("string::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "string", is_string, node, ctx);
            Kind::Int
        }
        "semver::inc::major" | "semver::inc::minor" | "semver::inc::patch" => {
            let full_name = format!("string::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }
        "semver::set::major" | "semver::set::minor" | "semver::set::patch" => {
            let full_name = format!("string::{}", func);
            expect_args(&full_name, arg_types, 2, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "string", is_string, node, ctx);
            Kind::String
        }

        _ => {
            // Try camelCase alias lookup before giving up
            if let Some(snake) = camel_alias(func) {
                return resolve(snake, arg_types, node, ctx);
            }
            // Try generic camelCase -> snake_case conversion
            let snake = camel_to_snake(func);
            if snake != func {
                // Avoid infinite recursion: only recurse if the conversion changed something
                return resolve(&snake, arg_types, node, ctx);
            }
            emit_unknown_function(
                &format!("string::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}
