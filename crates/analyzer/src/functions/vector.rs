use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_args};

const KNOWN_FUNCS: &[&str] = &[
    "add", "subtract", "multiply", "divide", "normalize", "scale",
    "project", "cross", "dot", "angle", "magnitude",
    "distance::chebyshev", "distance::euclidean", "distance::hamming",
    "distance::manhattan", "distance::minkowski", "distance::knn",
    "distance::mahalanobis",
    "similarity::cosine", "similarity::jaccard", "similarity::pearson",
    "similarity::spearman",
];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        // --- Vector arithmetic (2 vectors → vector) ---
        "add" | "subtract" | "multiply" | "divide" => {
            expect_args(&format!("vector::{}", func), arg_types, 2, node, ctx);
            Kind::Array(Box::new(Kind::Float), None)
        }

        // --- Normalize (1 vector → vector) ---
        "normalize" => {
            expect_args("vector::normalize", arg_types, 1, node, ctx);
            Kind::Array(Box::new(Kind::Float), None)
        }

        // --- Scale (vector, scalar → vector) ---
        "scale" => {
            expect_args("vector::scale", arg_types, 2, node, ctx);
            Kind::Array(Box::new(Kind::Float), None)
        }

        // --- Project (2 vectors → vector) ---
        "project" => {
            expect_args("vector::project", arg_types, 2, node, ctx);
            Kind::Array(Box::new(Kind::Float), None)
        }

        // --- Cross product (2 vectors → vector) ---
        "cross" => {
            expect_args("vector::cross", arg_types, 2, node, ctx);
            Kind::Array(Box::new(Kind::Float), None)
        }

        // --- Dot product (2 vectors → float) ---
        "dot" => {
            expect_args("vector::dot", arg_types, 2, node, ctx);
            Kind::Float
        }

        // --- Angle (2 vectors → float) ---
        "angle" => {
            expect_args("vector::angle", arg_types, 2, node, ctx);
            Kind::Float
        }

        // --- Magnitude (1 vector → float) ---
        "magnitude" => {
            expect_args("vector::magnitude", arg_types, 1, node, ctx);
            Kind::Float
        }

        // --- Distance functions (2 vectors → float) ---
        "distance::chebyshev" | "distance::euclidean" | "distance::hamming"
        | "distance::manhattan" | "distance::minkowski" => {
            expect_args(&format!("vector::{}", func), arg_types, 2, node, ctx);
            Kind::Float
        }
        "distance::knn" => {
            expect_args("vector::distance::knn", arg_types, 2, node, ctx);
            Kind::Float
        }
        "distance::mahalanobis" => {
            expect_args("vector::distance::mahalanobis", arg_types, 2, node, ctx);
            Kind::Float
        }

        // --- Similarity functions (2 vectors → float) ---
        "similarity::cosine" | "similarity::jaccard" | "similarity::pearson"
        | "similarity::spearman" => {
            expect_args(&format!("vector::{}", func), arg_types, 2, node, ctx);
            Kind::Float
        }

        _ => {
            emit_unknown_function(
                &format!("vector::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}
