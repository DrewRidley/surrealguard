use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_arg_type, expect_args, is_string};

const KNOWN_FUNCS: &[&str] = &[
    "array", "bool", "bytes", "datetime", "decimal", "duration", "float",
    "geometry", "int", "number", "point", "range", "record", "string",
    "table", "thing", "uuid", "field", "fields", "regex",
    "is::array", "is::bool", "is::bytes", "is::collection", "is::datetime",
    "is::decimal", "is::duration", "is::float", "is::geometry", "is::int",
    "is::line", "is::multiline", "is::multipoint", "is::multipolygon", "is::none",
    "is::null", "is::number", "is::object", "is::point", "is::polygon",
    "is::record", "is::string", "is::uuid",
];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        // --- Conversion functions (1 arg each) ---
        "array" => {
            expect_args("type::array", arg_types, 1, node, ctx);
            Kind::Array(Box::new(Kind::Any), None)
        }
        "bool" => {
            expect_args("type::bool", arg_types, 1, node, ctx);
            Kind::Bool
        }
        "bytes" => {
            expect_args("type::bytes", arg_types, 1, node, ctx);
            Kind::Bytes
        }
        "datetime" => {
            expect_args("type::datetime", arg_types, 1, node, ctx);
            Kind::Datetime
        }
        "decimal" => {
            expect_args("type::decimal", arg_types, 1, node, ctx);
            Kind::Decimal
        }
        "duration" => {
            expect_args("type::duration", arg_types, 1, node, ctx);
            Kind::Duration
        }
        "float" => {
            expect_args("type::float", arg_types, 1, node, ctx);
            Kind::Float
        }
        "geometry" => {
            expect_args("type::geometry", arg_types, 1, node, ctx);
            Kind::Geometry(vec![])
        }
        "int" => {
            expect_args("type::int", arg_types, 1, node, ctx);
            Kind::Int
        }
        "number" => {
            expect_args("type::number", arg_types, 1, node, ctx);
            Kind::Number
        }
        "point" => {
            expect_args("type::point", arg_types, 1, node, ctx);
            Kind::Geometry(vec!["point".into()])
        }
        "range" => {
            expect_args("type::range", arg_types, 1, node, ctx);
            Kind::Range
        }
        "record" => {
            expect_args("type::record", arg_types, 1, node, ctx);
            Kind::Record(vec![])
        }
        "string" => {
            expect_args("type::string", arg_types, 1, node, ctx);
            Kind::String
        }
        "table" => {
            expect_args("type::table", arg_types, 1, node, ctx);
            Kind::String
        }
        "thing" => {
            expect_args("type::thing", arg_types, 2, node, ctx);
            Kind::Record(vec![])
        }
        "uuid" => {
            expect_args("type::uuid", arg_types, 1, node, ctx);
            Kind::Uuid
        }

        // --- field / fields ---
        "field" => {
            expect_args("type::field", arg_types, 1, node, ctx);
            Kind::String
        }
        "fields" => {
            expect_args("type::fields", arg_types, 1, node, ctx);
            Kind::Array(Box::new(Kind::String), None)
        }

        "regex" => {
            expect_args("type::regex", arg_types, 1, node, ctx);
            expect_arg_type("type::regex", arg_types, 0, "string", is_string, node, ctx);
            crate::types::regex_kind()
        }

        // --- is:: type checks (all 1 arg → bool) ---
        "is::array" | "is::bool" | "is::bytes" | "is::collection" | "is::datetime"
        | "is::decimal" | "is::duration" | "is::float" | "is::geometry" | "is::int"
        | "is::line" | "is::multiline" | "is::multipoint" | "is::multipolygon" | "is::none"
        | "is::null" | "is::number" | "is::object" | "is::point" | "is::polygon"
        | "is::record" | "is::string" | "is::uuid" => {
            expect_args(&format!("type::{}", func), arg_types, 1, node, ctx);
            Kind::Bool
        }

        _ => {
            emit_unknown_function(
                &format!("type::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn type_regex_returns_regex() {
        let result = crate::analyze("LET $x = type::regex('test.*');").unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }

    #[test]
    fn type_range_returns_range() {
        let result = crate::analyze("LET $x = type::range(1..10);").unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }
}
