use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_arg_type, expect_args, is_array, is_numeric};

const KNOWN_FUNCS: &[&str] = &[
    "e", "pi", "inf", "neg_inf", "tau", "ln_10", "ln_2", "log10_2", "log10_e",
    "log2_10", "log2_e", "frac_1_pi", "frac_1_sqrt_2", "frac_2_pi",
    "frac_2_sqrt_pi", "frac_pi_2", "frac_pi_3", "frac_pi_4", "frac_pi_6",
    "frac_pi_8", "sqrt_2",
    "abs", "ceil", "floor", "round", "sqrt", "ln", "log10", "log2",
    "acos", "acot", "asin", "atan", "cos", "cot", "sin", "tan",
    "deg2rad", "rad2deg", "log", "pow", "fixed", "lerp", "lerpangle", "clamp", "sign",
    "bottom", "top", "max", "min", "mean", "median", "midhinge", "trimean",
    "spread", "interquartile", "variance", "stddev", "mode",
    "nearestrank", "percentile", "product", "sum",
];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        // --- Constants (0 args) ---
        "e" | "pi" | "inf" | "neg_inf" | "tau" | "ln_10" | "ln_2" | "log10_2" | "log10_e"
        | "log2_10" | "log2_e" | "frac_1_pi" | "frac_1_sqrt_2" | "frac_2_pi"
        | "frac_2_sqrt_pi" | "frac_pi_2" | "frac_pi_3" | "frac_pi_4" | "frac_pi_6"
        | "frac_pi_8" | "sqrt_2" => {
            expect_args(&format!("math::{}", func), arg_types, 0, node, ctx);
            Kind::Float
        }

        // --- Single-arg numeric → float ---
        "abs" | "ceil" | "floor" | "round" | "sqrt" | "ln" | "log10" | "log2" => {
            let full_name = format!("math::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "numeric", is_numeric, node, ctx);
            Kind::Float
        }

        // --- Trig (1 arg) ---
        "acos" | "acot" | "asin" | "atan" | "cos" | "cot" | "sin" | "tan" => {
            let full_name = format!("math::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "numeric", is_numeric, node, ctx);
            Kind::Float
        }

        // --- Angle conversion (1 arg) ---
        "deg2rad" | "rad2deg" => {
            let full_name = format!("math::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "numeric", is_numeric, node, ctx);
            Kind::Float
        }

        // --- Two-arg functions ---
        "log" => {
            expect_args("math::log", arg_types, 2, node, ctx);
            expect_arg_type("math::log", arg_types, 0, "numeric", is_numeric, node, ctx);
            Kind::Float
        }
        "pow" => {
            expect_args("math::pow", arg_types, 2, node, ctx);
            expect_arg_type("math::pow", arg_types, 0, "numeric", is_numeric, node, ctx);
            Kind::Number
        }
        "fixed" => {
            expect_args("math::fixed", arg_types, 2, node, ctx);
            expect_arg_type("math::fixed", arg_types, 0, "numeric", is_numeric, node, ctx);
            Kind::Float
        }
        "lerp" => {
            expect_args("math::lerp", arg_types, 3, node, ctx);
            expect_arg_type("math::lerp", arg_types, 0, "numeric", is_numeric, node, ctx);
            Kind::Float
        }
        "lerpangle" => {
            expect_args("math::lerpangle", arg_types, 3, node, ctx);
            expect_arg_type("math::lerpangle", arg_types, 0, "numeric", is_numeric, node, ctx);
            Kind::Float
        }
        "clamp" => {
            expect_args("math::clamp", arg_types, 3, node, ctx);
            expect_arg_type("math::clamp", arg_types, 0, "numeric", is_numeric, node, ctx);
            Kind::Number
        }

        // --- Sign (1 arg → int) ---
        "sign" => {
            expect_args("math::sign", arg_types, 1, node, ctx);
            expect_arg_type("math::sign", arg_types, 0, "numeric", is_numeric, node, ctx);
            Kind::Int
        }

        // --- Aggregate / statistical (1 arg: array or variadic) ---
        "bottom" => {
            expect_args("math::bottom", arg_types, 2, node, ctx);
            expect_arg_type("math::bottom", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(Kind::Number), None)
        }
        "top" => {
            expect_args("math::top", arg_types, 2, node, ctx);
            expect_arg_type("math::top", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(Kind::Number), None)
        }
        "max" | "min" => {
            let full_name = format!("math::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "array", is_array, node, ctx);
            Kind::Number
        }
        "mean" | "median" | "midhinge" | "trimean" | "spread" | "interquartile" | "variance"
        | "stddev" => {
            let full_name = format!("math::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "array", is_array, node, ctx);
            Kind::Number
        }
        "mode" => {
            expect_args("math::mode", arg_types, 1, node, ctx);
            expect_arg_type("math::mode", arg_types, 0, "array", is_array, node, ctx);
            Kind::Number
        }
        "nearestrank" | "percentile" => {
            let full_name = format!("math::{}", func);
            expect_args(&full_name, arg_types, 2, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "array", is_array, node, ctx);
            Kind::Number
        }
        "product" | "sum" => {
            let full_name = format!("math::{}", func);
            expect_args(&full_name, arg_types, 1, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "array", is_array, node, ctx);
            Kind::Number
        }

        _ => {
            emit_unknown_function(
                &format!("math::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Number
        }
    }
}
