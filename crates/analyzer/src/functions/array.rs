use tree_sitter::Node;

use crate::context::Context;
use crate::types::{Kind, KindExt};

use super::{emit_unknown_function, expect_arg_type, expect_args, expect_args_range, expect_min_args, is_array, is_numeric};

const KNOWN_FUNCS: &[&str] = &[
    "all", "every", "any", "includes", "some", "is_empty", "matches",
    "add", "append", "combine", "complement", "concat", "difference",
    "distinct", "fill", "filter", "insert", "intersect", "prepend",
    "push", "remove", "repeat", "reverse", "shuffle", "slice", "sort",
    "sort::asc", "sort::desc", "swap", "unique", "union",
    "boolean_and", "boolean_not", "boolean_or", "boolean_xor",
    "logical_and", "logical_or", "logical_xor",
    "clump", "windows", "transpose", "range", "map",
    "filter_index", "find", "find_index", "index_of",
    "fold", "reduce", "flatten", "group", "join",
    "first", "last", "pop", "at", "len", "knn",
];

pub fn resolve(func: &str, arg_types: &[Kind], node: &Node, ctx: &mut Context) -> Kind {
    let first = arg_types.first().cloned().unwrap_or(Kind::Any);

    // Extract the inner element type from array arguments
    let element_type = match &first {
        Kind::Array(inner, _) => inner.as_ref().clone(),
        _ => Kind::Any,
    };

    match func {
        // --- Bool returns ---
        "all" | "every" => {
            expect_args("array::all", arg_types, 1, node, ctx);
            expect_arg_type("array::all", arg_types, 0, "array", is_array, node, ctx);
            Kind::Bool
        }
        "any" | "includes" | "some" => {
            expect_args("array::any", arg_types, 1, node, ctx);
            expect_arg_type("array::any", arg_types, 0, "array", is_array, node, ctx);
            Kind::Bool
        }
        "is_empty" => {
            expect_args("array::is_empty", arg_types, 1, node, ctx);
            expect_arg_type("array::is_empty", arg_types, 0, "array", is_array, node, ctx);
            Kind::Bool
        }
        "matches" => {
            expect_args("array::matches", arg_types, 2, node, ctx);
            expect_arg_type("array::matches", arg_types, 0, "array", is_array, node, ctx);
            Kind::Bool
        }

        // --- Array returns (preserve element type) ---
        "add" => {
            expect_args("array::add", arg_types, 2, node, ctx);
            expect_arg_type("array::add", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "append" => {
            expect_args("array::append", arg_types, 2, node, ctx);
            expect_arg_type("array::append", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "combine" => {
            expect_args("array::combine", arg_types, 2, node, ctx);
            expect_arg_type("array::combine", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "complement" => {
            expect_min_args("array::complement", arg_types, 2, node, ctx);
            expect_arg_type("array::complement", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "concat" => {
            expect_min_args("array::concat", arg_types, 2, node, ctx);
            expect_arg_type("array::concat", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "difference" => {
            expect_args("array::difference", arg_types, 2, node, ctx);
            expect_arg_type("array::difference", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "distinct" => {
            expect_args("array::distinct", arg_types, 1, node, ctx);
            expect_arg_type("array::distinct", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "fill" => {
            expect_args("array::fill", arg_types, 2, node, ctx);
            expect_arg_type("array::fill", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "filter" => {
            expect_args("array::filter", arg_types, 2, node, ctx);
            expect_arg_type("array::filter", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "insert" => {
            expect_args_range("array::insert", arg_types, 2, 3, node, ctx);
            expect_arg_type("array::insert", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "intersect" => {
            expect_args("array::intersect", arg_types, 2, node, ctx);
            expect_arg_type("array::intersect", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "prepend" => {
            expect_args("array::prepend", arg_types, 2, node, ctx);
            expect_arg_type("array::prepend", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "push" => {
            expect_args("array::push", arg_types, 2, node, ctx);
            expect_arg_type("array::push", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "remove" => {
            expect_args("array::remove", arg_types, 2, node, ctx);
            expect_arg_type("array::remove", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "repeat" => {
            expect_args("array::repeat", arg_types, 2, node, ctx);
            expect_arg_type("array::repeat", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "reverse" => {
            expect_args("array::reverse", arg_types, 1, node, ctx);
            expect_arg_type("array::reverse", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "shuffle" => {
            expect_args("array::shuffle", arg_types, 1, node, ctx);
            expect_arg_type("array::shuffle", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "slice" => {
            expect_args_range("array::slice", arg_types, 2, 3, node, ctx);
            expect_arg_type("array::slice", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "sort" => {
            expect_args_range("array::sort", arg_types, 1, 2, node, ctx);
            expect_arg_type("array::sort", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "sort::asc" => {
            expect_args("array::sort::asc", arg_types, 1, node, ctx);
            expect_arg_type("array::sort::asc", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "sort::desc" => {
            expect_args("array::sort::desc", arg_types, 1, node, ctx);
            expect_arg_type("array::sort::desc", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "swap" => {
            expect_args("array::swap", arg_types, 3, node, ctx);
            expect_arg_type("array::swap", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "unique" => {
            expect_args("array::unique", arg_types, 1, node, ctx);
            expect_arg_type("array::unique", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "union" => {
            expect_args("array::union", arg_types, 2, node, ctx);
            expect_arg_type("array::union", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }

        // --- Array returns (different element type) ---
        "boolean_and" => {
            expect_args("array::boolean_and", arg_types, 2, node, ctx);
            expect_arg_type("array::boolean_and", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(Kind::Bool), None)
        }
        "boolean_not" => {
            expect_args("array::boolean_not", arg_types, 2, node, ctx);
            expect_arg_type("array::boolean_not", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(Kind::Bool), None)
        }
        "boolean_or" => {
            expect_args("array::boolean_or", arg_types, 2, node, ctx);
            expect_arg_type("array::boolean_or", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(Kind::Bool), None)
        }
        "boolean_xor" => {
            expect_args("array::boolean_xor", arg_types, 2, node, ctx);
            expect_arg_type("array::boolean_xor", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(Kind::Bool), None)
        }
        "logical_and" => {
            expect_args("array::logical_and", arg_types, 2, node, ctx);
            expect_arg_type("array::logical_and", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(Kind::Bool), None)
        }
        "logical_or" => {
            expect_args("array::logical_or", arg_types, 2, node, ctx);
            expect_arg_type("array::logical_or", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(Kind::Bool), None)
        }
        "logical_xor" => {
            expect_args("array::logical_xor", arg_types, 2, node, ctx);
            expect_arg_type("array::logical_xor", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(Kind::Bool), None)
        }
        "clump" => {
            expect_args("array::clump", arg_types, 2, node, ctx);
            expect_arg_type("array::clump", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(Kind::Array(Box::new(element_type), None)), None)
        }
        "windows" => {
            expect_args("array::windows", arg_types, 2, node, ctx);
            expect_arg_type("array::windows", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(Kind::Array(Box::new(element_type), None)), None)
        }
        "transpose" => {
            expect_args("array::transpose", arg_types, 1, node, ctx);
            expect_arg_type("array::transpose", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(Kind::Array(Box::new(Kind::Any), None)), None)
        }
        "range" => {
            expect_args_range("array::range", arg_types, 2, 3, node, ctx);
            Kind::Array(Box::new(Kind::Number), None)
        }
        "map" => {
            expect_args("array::map", arg_types, 2, node, ctx);
            expect_arg_type("array::map", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(Kind::Any), None)
        }
        "filter_index" => {
            expect_args("array::filter_index", arg_types, 2, node, ctx);
            expect_arg_type("array::filter_index", arg_types, 0, "array", is_array, node, ctx);
            Kind::Array(Box::new(Kind::Int), None)
        }
        "find_index" | "index_of" => {
            let full_name = format!("array::{}", func);
            expect_args(&full_name, arg_types, 2, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "array", is_array, node, ctx);
            Kind::Int.optional()
        }

        // --- Element returns ---
        "at" => {
            expect_args("array::at", arg_types, 2, node, ctx);
            expect_arg_type("array::at", arg_types, 0, "array", is_array, node, ctx);
            element_type.optional()
        }
        "find" => {
            expect_args("array::find", arg_types, 2, node, ctx);
            expect_arg_type("array::find", arg_types, 0, "array", is_array, node, ctx);
            element_type.optional()
        }
        "first" => {
            expect_args("array::first", arg_types, 1, node, ctx);
            expect_arg_type("array::first", arg_types, 0, "array", is_array, node, ctx);
            element_type.optional()
        }
        "last" => {
            expect_args("array::last", arg_types, 1, node, ctx);
            expect_arg_type("array::last", arg_types, 0, "array", is_array, node, ctx);
            element_type.optional()
        }
        "max" => {
            expect_args("array::max", arg_types, 1, node, ctx);
            expect_arg_type("array::max", arg_types, 0, "array", is_array, node, ctx);
            element_type.optional()
        }
        "min" => {
            expect_args("array::min", arg_types, 1, node, ctx);
            expect_arg_type("array::min", arg_types, 0, "array", is_array, node, ctx);
            element_type.optional()
        }
        "pop" => {
            expect_args("array::pop", arg_types, 1, node, ctx);
            expect_arg_type("array::pop", arg_types, 0, "array", is_array, node, ctx);
            element_type.optional()
        }

        // --- Flatten ---
        "flatten" => {
            expect_args("array::flatten", arg_types, 1, node, ctx);
            expect_arg_type("array::flatten", arg_types, 0, "array", is_array, node, ctx);
            match &first {
                Kind::Array(inner, _) => match inner.as_ref() {
                    Kind::Array(nested, _) => Kind::Array(nested.clone(), None),
                    _ => first.clone(),
                },
                _ => Kind::Array(Box::new(Kind::Any), None),
            }
        }

        // --- Other ---
        "fold" | "reduce" => {
            let full_name = format!("array::{}", func);
            expect_args(&full_name, arg_types, 3, node, ctx);
            expect_arg_type(&full_name, arg_types, 0, "array", is_array, node, ctx);
            Kind::Any
        }
        "group" => {
            expect_args("array::group", arg_types, 1, node, ctx);
            expect_arg_type("array::group", arg_types, 0, "array", is_array, node, ctx);
            Kind::Object
        }
        "join" => {
            expect_args("array::join", arg_types, 2, node, ctx);
            expect_arg_type("array::join", arg_types, 0, "array", is_array, node, ctx);
            Kind::String
        }
        "knn" => {
            expect_args("array::knn", arg_types, 3, node, ctx);
            expect_arg_type("array::knn", arg_types, 0, "array", is_array, node, ctx);
            expect_arg_type("array::knn", arg_types, 2, "numeric", is_numeric, node, ctx);
            Kind::Array(Box::new(element_type), None)
        }
        "len" => {
            expect_args("array::len", arg_types, 1, node, ctx);
            expect_arg_type("array::len", arg_types, 0, "array", is_array, node, ctx);
            Kind::Int
        }

        _ => {
            emit_unknown_function(
                &format!("array::{}", func),
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
    use crate::diagnostic::Code;

    #[test]
    fn array_unique_returns_array() {
        let result = crate::analyze("LET $x = array::unique([1,2,3]);").unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }

    #[test]
    fn array_knn_returns_array() {
        let result = crate::analyze("LET $x = array::knn([1,2,3], 2, 1);").unwrap();
        let errors: Vec<_> = result.diagnostics.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
    }

    #[test]
    fn array_knn_wrong_arg_count() {
        let result = crate::analyze("LET $x = array::knn([1,2,3], 2);").unwrap();
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code == Code::WrongArgCount)
            .collect();
        assert!(
            !errors.is_empty(),
            "Expected wrong arg count for array::knn with 2 args, got: {:?}",
            result.diagnostics
        );
    }
}
