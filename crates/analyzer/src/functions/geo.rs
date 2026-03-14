use tree_sitter::Node;

use crate::context::Context;
use crate::types::Kind;

use super::{emit_unknown_function, expect_arg_type, expect_args, expect_args_range, is_geometry, is_string};

const KNOWN_FUNCS: &[&str] = &[
    "area", "bearing", "centroid", "distance",
    "hash::decode", "hash::encode", "is::valid",
];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    match func {
        "area" => {
            expect_args("geo::area", arg_types, 1, node, ctx);
            expect_arg_type("geo::area", arg_types, 0, "geometry", is_geometry, node, ctx);
            Kind::Float
        }
        "bearing" => {
            expect_args("geo::bearing", arg_types, 2, node, ctx);
            expect_arg_type("geo::bearing", arg_types, 0, "geometry", is_geometry, node, ctx);
            expect_arg_type("geo::bearing", arg_types, 1, "geometry", is_geometry, node, ctx);
            Kind::Float
        }
        "centroid" => {
            expect_args("geo::centroid", arg_types, 1, node, ctx);
            expect_arg_type("geo::centroid", arg_types, 0, "geometry", is_geometry, node, ctx);
            Kind::Geometry(vec!["point".into()])
        }
        "distance" => {
            expect_args("geo::distance", arg_types, 2, node, ctx);
            expect_arg_type("geo::distance", arg_types, 0, "geometry", is_geometry, node, ctx);
            expect_arg_type("geo::distance", arg_types, 1, "geometry", is_geometry, node, ctx);
            Kind::Float
        }
        "hash::decode" => {
            expect_args("geo::hash::decode", arg_types, 1, node, ctx);
            expect_arg_type("geo::hash::decode", arg_types, 0, "string", is_string, node, ctx);
            Kind::Geometry(vec!["point".into()])
        }
        "hash::encode" => {
            expect_args_range("geo::hash::encode", arg_types, 1, 2, node, ctx);
            expect_arg_type("geo::hash::encode", arg_types, 0, "geometry", is_geometry, node, ctx);
            Kind::String
        }
        "is::valid" => {
            expect_args("geo::is::valid", arg_types, 1, node, ctx);
            expect_arg_type("geo::is::valid", arg_types, 0, "geometry", is_geometry, node, ctx);
            Kind::Bool
        }

        _ => {
            emit_unknown_function(
                &format!("geo::{}", func),
                func,
                KNOWN_FUNCS,
                node,
                ctx,
            );
            Kind::Any
        }
    }
}
