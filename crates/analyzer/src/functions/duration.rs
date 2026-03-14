use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_arg_type, expect_args, is_duration, is_numeric};

const KNOWN_FUNCS: &[&str] = &[
    "from::days", "from::hours", "from::micros", "from::millis", "from::mins",
    "from::nanos", "from::secs", "from::weeks",
    "days", "hours", "micros", "millis", "mins", "nanos", "secs", "weeks", "years",
];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        // --- from:: conversions → Duration ---
        "from::days" | "from::hours" | "from::micros" | "from::millis" | "from::mins"
        | "from::nanos" | "from::secs" | "from::weeks" => {
            let full_name = format!("duration::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "numeric", is_numeric, node, ctx);
            Kind::Duration
        }

        // --- Duration → Int extractions ---
        "days" | "hours" | "micros" | "millis" | "mins" | "nanos" | "secs" | "weeks"
        | "years" => {
            let full_name = format!("duration::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "duration", is_duration, node, ctx);
            Kind::Int
        }

        _ => {
            emit_unknown_function(
                &format!("duration::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}
