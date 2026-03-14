use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_arg_type, expect_args, is_datetime};

const KNOWN_FUNCS: &[&str] = &[
    "now", "from::micros", "from::millis", "from::nanos", "from::secs",
    "from::unix", "from::ulid", "from::uuid",
    "ceil", "floor", "round", "group", "format", "timezone",
    "day", "hour", "minute", "second", "month", "year", "wday", "week", "yday",
    "unix", "micros", "millis", "nano", "is::leap_year", "max", "min",
];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        // --- Now (0 args) ---
        "now" => {
            expect_args("time::now", arg_types, 0, node, ctx);
            Kind::Datetime
        }

        // --- from:: conversions → Datetime ---
        "from::micros" | "from::millis" | "from::nanos" | "from::secs" | "from::unix" => {
            expect_args(&format!("time::{}", func), arg_types, 1, node, ctx);
            Kind::Datetime
        }
        "from::ulid" => {
            expect_args("time::from::ulid", arg_types, 1, node, ctx);
            Kind::Datetime
        }
        "from::uuid" => {
            expect_args("time::from::uuid", arg_types, 1, node, ctx);
            Kind::Datetime
        }

        // --- Datetime → Datetime rounding ---
        "ceil" | "floor" | "round" => {
            let full_name = format!("time::{}", func);
            expect_args(&full_name, arg_types, 2, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "datetime", is_datetime, node, ctx);
            Kind::Datetime
        }

        // --- Group ---
        "group" => {
            expect_args("time::group", arg_types, 2, node, ctx);
            expect_arg_type("time::group", arg_types, 0, "datetime", is_datetime, node, ctx);
            Kind::Datetime
        }

        // --- Format → String ---
        "format" => {
            expect_args("time::format", arg_types, 2, node, ctx);
            expect_arg_type("time::format", arg_types, 0, "datetime", is_datetime, node, ctx);
            Kind::String
        }

        // --- Timezone → String ---
        "timezone" => {
            expect_args("time::timezone", arg_types, 0, node, ctx);
            Kind::String
        }

        // --- Int extractions ---
        "day" | "hour" | "minute" | "second" | "month" | "year" | "wday" | "week" | "yday" => {
            let full_name = format!("time::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "datetime", is_datetime, node, ctx);
            Kind::Int
        }

        // --- Unix timestamp ---
        "unix" => {
            expect_args("time::unix", arg_types, 1, node, ctx);
            expect_arg_type("time::unix", arg_types, 0, "datetime", is_datetime, node, ctx);
            Kind::Int
        }

        // --- Sub-second extractions → Int ---
        "micros" | "millis" | "nano" => {
            let full_name = format!("time::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "datetime", is_datetime, node, ctx);
            Kind::Int
        }

        // --- is:: checks ---
        "is::leap_year" => {
            expect_args("time::is::leap_year", arg_types, 1, node, ctx);
            expect_arg_type("time::is::leap_year", arg_types, 0, "datetime", is_datetime, node, ctx);
            Kind::Bool
        }

        // --- Min / Max ---
        "max" | "min" => {
            let full_name = format!("time::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "datetime", is_datetime, node, ctx);
            Kind::Datetime
        }

        _ => {
            emit_unknown_function(
                &format!("time::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}
