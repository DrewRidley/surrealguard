/// Built-in function type signatures and argument validation.
///
/// Dispatches by namespace (array::, string::, math::, etc.) and
/// returns the resolved return type for each function.
use tree_sitter::Node;

use crate::context::Context;
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::find_all;
use crate::span::Span;
use crate::types::{Kind, KindExt, Literal};

/// Find the function name node within a function call for precise spans.
pub(crate) fn find_fn_name_node<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "function_name" | "keyword" => return Some(child),
            _ => {}
        }
    }
    None
}

/// Find the argument list node (everything between parens) within a function call.
pub(crate) fn find_arg_list_span(node: &Node) -> Option<Span> {
    let mut cursor = node.walk();
    let mut open = None;
    let mut close = None;
    for child in node.children(&mut cursor) {
        if child.kind() == "(" {
            open = Some(child);
        } else if child.kind() == ")" {
            close = Some(child);
        }
    }
    if let (Some(o), Some(c)) = (open, close) {
        Some(Span {
            start: o.start_byte() as u32,
            end: c.end_byte() as u32,
        })
    } else {
        None
    }
}

/// Find the Nth argument node within a function call.
pub(crate) fn find_nth_arg_node<'a>(node: &Node<'a>, index: usize) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let mut in_args = false;
    let mut arg_index = 0;
    for child in node.named_children(&mut cursor) {
        // Skip the function name identifiers
        if child.kind() == "identifier" || child.kind() == "function_name" {
            continue;
        }
        // Arguments come after the function name
        if !in_args {
            in_args = true;
        }
        if in_args {
            // Each named child that's not a keyword/comma is an argument
            if child.kind() != "," {
                if arg_index == index {
                    return Some(child);
                }
                arg_index += 1;
            }
        }
    }
    // Try find_all for "value" nodes as arguments
    let values = find_all(node, "value");
    values.get(index).copied()
}

mod array;
mod bytes;
mod crypto;
mod duration;
mod encoding;
mod geo;
mod http;
mod math;
mod meta;
mod object;
mod parse;
mod rand;
mod record;
mod search;
mod session;
mod string;
mod time;
mod type_fns;
mod value;
mod vector;

/// Check that a function received exactly `expected` arguments.
/// Returns `true` if the count is correct, `false` otherwise (and emits a diagnostic).
pub fn expect_args(
    name: &str,
    args: &[Kind],
    expected: usize,
    node: &Node,
    ctx: &mut Context,
) -> bool {
    if args.len() != expected {
        let span = find_arg_list_span(node)
            .or_else(|| find_fn_name_node(node).map(|n| Span::from_node(&n)))
            .unwrap_or_else(|| Span::from_node(node));
        ctx.emit(Diagnostic::error(
            span,
            Code::WrongArgCount,
            format!(
                "`{}` takes {} argument{} but {} {} supplied",
                name,
                expected,
                if expected == 1 { "" } else { "s" },
                args.len(),
                if args.len() == 1 { "was" } else { "were" }
            ),
        ));
        false
    } else {
        true
    }
}

/// Check that a function received at least `min` arguments.
/// Returns `true` if the count is sufficient, `false` otherwise (and emits a diagnostic).
pub fn expect_min_args(
    name: &str,
    args: &[Kind],
    min: usize,
    node: &Node,
    ctx: &mut Context,
) -> bool {
    if args.len() < min {
        let span = find_arg_list_span(node)
            .or_else(|| find_fn_name_node(node).map(|n| Span::from_node(&n)))
            .unwrap_or_else(|| Span::from_node(node));
        ctx.emit(Diagnostic::error(
            span,
            Code::WrongArgCount,
            format!(
                "`{}` takes at least {} argument{} but {} {} supplied",
                name,
                min,
                if min == 1 { "" } else { "s" },
                args.len(),
                if args.len() == 1 { "was" } else { "were" }
            ),
        ));
        false
    } else {
        true
    }
}

/// Check that a function received between `min` and `max` arguments (inclusive).
/// Returns `true` if the count is in range, `false` otherwise (and emits a diagnostic).
pub fn expect_args_range(
    name: &str,
    args: &[Kind],
    min: usize,
    max: usize,
    node: &Node,
    ctx: &mut Context,
) -> bool {
    if args.len() < min || args.len() > max {
        let span = find_arg_list_span(node)
            .or_else(|| find_fn_name_node(node).map(|n| Span::from_node(&n)))
            .unwrap_or_else(|| Span::from_node(node));
        ctx.emit(Diagnostic::error(
            span,
            Code::WrongArgCount,
            format!(
                "`{}` takes {}-{} arguments but {} {} supplied",
                name,
                min,
                max,
                args.len(),
                if args.len() == 1 { "was" } else { "were" }
            ),
        ));
        false
    } else {
        true
    }
}

/// Validate that an argument has an expected type category.
/// Returns true if valid, false if error was emitted.
pub fn expect_arg_type(
    name: &str,
    arg_types: &[Kind],
    index: usize,
    expected: &str,
    check: impl Fn(&Kind) -> bool,
    node: &Node,
    ctx: &mut Context,
) -> bool {
    if let Some(actual) = arg_types.get(index) {
        if actual.is_any() || check(actual) {
            return true;
        }
        let span = find_nth_arg_node(node, index)
            .map(|n| Span::from_node(&n))
            .unwrap_or_else(|| Span::from_node(node));
        let mut diag = Diagnostic::warning(
            span,
            Code::WrongArgType,
            format!(
                "`{}` expects `{}` for argument {}, found `{}`",
                name,
                expected,
                index + 1,
                actual
            ),
        );
        // Add type conversion suggestions for common mismatches
        if expected == "string" && !matches!(actual, Kind::String) {
            diag = diag.with_suggestion(format!(
                "consider converting with `<string>value` or `string::from(value)`"
            ));
        } else if (expected == "int" || expected == "number" || expected == "numeric")
            && matches!(actual, Kind::String)
        {
            diag = diag.with_suggestion(format!(
                "consider converting with `<int>value` or `<number>value`"
            ));
        }
        ctx.emit(diag);
        return false;
    }
    true
}

pub(crate) fn is_numeric(k: &Kind) -> bool {
    matches!(k, Kind::Int | Kind::Float | Kind::Decimal | Kind::Number)
}

pub(crate) fn is_string(k: &Kind) -> bool {
    matches!(k, Kind::String)
}

pub(crate) fn is_array(k: &Kind) -> bool {
    matches!(k, Kind::Array(_, _))
}

pub(crate) fn is_object(k: &Kind) -> bool {
    matches!(k, Kind::Object | Kind::Literal(Literal::Object(_)))
}

#[allow(dead_code)]
pub(crate) fn is_bool(k: &Kind) -> bool {
    matches!(k, Kind::Bool)
}

pub(crate) fn is_duration(k: &Kind) -> bool {
    matches!(k, Kind::Duration)
}

pub(crate) fn is_datetime(k: &Kind) -> bool {
    matches!(k, Kind::Datetime)
}

pub(crate) fn is_geometry(k: &Kind) -> bool {
    matches!(k, Kind::Geometry(_))
}

pub(crate) fn is_record(k: &Kind) -> bool {
    matches!(k, Kind::Record(_))
}

/// Known built-in namespaces for suggesting corrections.
const KNOWN_NAMESPACES: &[&str] = &[
    "array", "bytes", "crypto", "duration", "encoding", "geo", "http",
    "math", "meta", "object", "parse", "rand", "record", "search",
    "session", "string", "time", "type", "value", "vector",
];

/// Known top-level functions for suggesting corrections.
const KNOWN_TOP_LEVEL: &[&str] = &["count", "sleep", "not", "throw", "type_of"];

/// Compute the Levenshtein edit distance between two strings.
fn edit_distance(a: &str, b: &str) -> usize {
    let a_len = a.len();
    let b_len = b.len();
    if a_len == 0 { return b_len; }
    if b_len == 0 { return a_len; }

    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0; b_len + 1];

    for (i, ca) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.chars().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (prev[j] + cost)
                .min(prev[j + 1] + 1)
                .min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b_len]
}

/// Find the closest match from `candidates` to `name`, returning it if the
/// edit distance is at most `max_dist`.
pub(crate) fn suggest_similar<'a>(name: &str, candidates: &[&'a str], max_dist: usize) -> Option<&'a str> {
    candidates
        .iter()
        .filter_map(|c| {
            let d = edit_distance(name, c);
            if d > 0 && d <= max_dist { Some((*c, d)) } else { None }
        })
        .min_by_key(|(_, d)| *d)
        .map(|(c, _)| c)
}

/// Emit a `FunctionNotFound` diagnostic with a "did you mean" suggestion when
/// a close match exists.
pub(crate) fn emit_unknown_function(
    full_name: &str,
    func_part: &str,
    candidates: &[&str],
    node: &Node,
    ctx: &mut Context,
) {
    let span = find_fn_name_node(node)
        .map(|n| Span::from_node(&n))
        .unwrap_or_else(|| Span::from_node(node));
    let mut diag = Diagnostic::warning(
        span,
        Code::FunctionNotFound,
        format!("unknown function `{}`", full_name),
    );
    // Suggest a similar function name if one is close (edit distance <= 3)
    if let Some(similar) = suggest_similar(func_part, candidates, 3) {
        let ns = full_name.rsplitn(2, "::").last().unwrap_or("");
        let suggested_full = if ns.is_empty() || ns == full_name {
            similar.to_string()
        } else {
            format!("{}::{}", ns, similar)
        };
        diag = diag.with_suggestion(format!("did you mean `{}`?", suggested_full));
    }
    ctx.emit(diag);
}

/// Resolve the return type of a built-in function call.
pub fn resolve_builtin(
    name: &str,
    arg_types: &[Kind],
    node: &Node,
    ctx: &mut Context,
) -> Kind {
    // Split namespace::function
    let parts: Vec<&str> = name.splitn(2, "::").collect();

    if parts.len() == 2 {
        let namespace = parts[0];
        let func = parts[1];

        match namespace {
            "array" => array::resolve(func, arg_types, node, ctx),
            "bytes" => bytes::resolve(func, arg_types, node, ctx),
            "crypto" => crypto::resolve(func, arg_types, node, ctx),
            "duration" => duration::resolve(func, arg_types, node, ctx),
            "encoding" => encoding::resolve(func, arg_types, node, ctx),
            "geo" => geo::resolve(func, arg_types, node, ctx),
            "http" => http::resolve(func, arg_types, node, ctx),
            "math" => math::resolve(func, arg_types, node, ctx),
            "meta" => meta::resolve(func, arg_types, node, ctx),
            "object" => object::resolve(func, arg_types, node, ctx),
            "parse" => parse::resolve(func, arg_types, node, ctx),
            "rand" => rand::resolve(func, arg_types, node, ctx),
            "record" => record::resolve(func, arg_types, node, ctx),
            "search" => search::resolve(func, arg_types, node, ctx),
            "session" => session::resolve(func, arg_types, node, ctx),
            "string" => string::resolve(func, arg_types, node, ctx),
            "time" => time::resolve(func, arg_types, node, ctx),
            "type" => type_fns::resolve(func, arg_types, node, ctx),
            "value" => value::resolve(func, arg_types, node, ctx),
            "vector" => vector::resolve(func, arg_types, node, ctx),
            _ => {
                let span = find_fn_name_node(node)
                    .map(|n| Span::from_node(&n))
                    .unwrap_or_else(|| Span::from_node(node));
                let mut diag = Diagnostic::warning(
                    span,
                    Code::FunctionNotFound,
                    format!("unknown function namespace `{}`", namespace),
                );
                if let Some(similar) = suggest_similar(namespace, KNOWN_NAMESPACES, 3) {
                    diag = diag.with_suggestion(format!(
                        "did you mean `{}::{}`?", similar, func
                    ));
                }
                ctx.emit(diag);
                Kind::Any
            }
        }
    } else {
        // Top-level functions
        match name {
            "count" => {
                // count() or count(array) or count(value)
                Kind::Int
            }
            "sleep" => {
                expect_args("sleep", arg_types, 1, node, ctx);
                Kind::Null
            }
            "not" => {
                expect_args("not", arg_types, 1, node, ctx);
                Kind::Bool
            }
            "throw" => {
                expect_args("throw", arg_types, 1, node, ctx);
                Kind::Null
            }
            "type_of" => {
                expect_args("type_of", arg_types, 1, node, ctx);
                Kind::String
            }
            _ => {
                emit_unknown_function(name, name, KNOWN_TOP_LEVEL, node, ctx);
                Kind::Any
            }
        }
    }
}
