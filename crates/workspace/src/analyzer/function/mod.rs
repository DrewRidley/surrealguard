//! Built-in SurrealQL function analyzers.
//!
//! `analyze_builtin_function` is the namespace entrypoint. It extracts the
//! function path from the source text, routes to a category `mod.rs`, and the
//! category fans out to one analyzer function per documented built-in function.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod api;
pub mod array;
pub mod bytes;
pub mod count;
pub mod crypto;
pub mod duration;
pub mod encoding;
pub mod file;
pub mod geo;
pub mod http;
pub mod math;
pub mod meta;
pub mod not;
pub mod object;
pub mod parse;
pub mod rand;
pub mod record;
pub mod search;
pub mod sequence;
pub mod session;
pub mod set;
pub(crate) mod signature;
pub mod sleep;
pub mod string;
pub mod time;
pub mod type_;
pub mod value;
pub mod vector;

pub fn analyze_builtin_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    // `call.path` is pre-normalized by lowering (`type::is::record` ->
    // `type::is_record`); custom `fn::*` functions fall through to `Any`.
    let path = call.path.node.as_str();

    match path.split("::").next().unwrap_or_default() {
        "api" => api::analyze_api_function(ctx, call, path, args),
        "array" => array::analyze_array_function(ctx, call, path, args),
        "bytes" => bytes::analyze_bytes_function(ctx, call, path, args),
        "count" => count::analyze_count_function(ctx, call, path, args),
        "crypto" => crypto::analyze_crypto_function(ctx, call, path, args),
        "duration" => duration::analyze_duration_function(ctx, call, path, args),
        "encoding" => encoding::analyze_encoding_function(ctx, call, path, args),
        "file" => file::analyze_file_function(ctx, call, path, args),
        "geo" => geo::analyze_geo_function(ctx, call, path, args),
        "http" => http::analyze_http_function(ctx, call, path, args),
        "math" => math::analyze_math_function(ctx, call, path, args),
        "meta" => meta::analyze_meta_function(ctx, call, path, args),
        "not" => not::analyze_not_function(ctx, call, path, args),
        "object" => object::analyze_object_function(ctx, call, path, args),
        "parse" => parse::analyze_parse_function(ctx, call, path, args),
        "rand" => rand::analyze_rand_function(ctx, call, path, args),
        "record" => record::analyze_record_function(ctx, call, path, args),
        "search" => search::analyze_search_function(ctx, call, path, args),
        "sequence" => sequence::analyze_sequence_function(ctx, call, path, args),
        "session" => session::analyze_session_function(ctx, call, path, args),
        "set" => set::analyze_set_function(ctx, call, path, args),
        "sleep" => sleep::analyze_sleep_function(ctx, call, path, args),
        "string" => string::analyze_string_function(ctx, call, path, args),
        "time" => time::analyze_time_function(ctx, call, path, args),
        "type" => type_::analyze_type_function(ctx, call, path, args),
        "value" => value::analyze_value_function(ctx, call, path, args),
        "vector" => vector::analyze_vector_function(ctx, call, path, args),
        // User-defined functions carry their declared return type on the
        // schema; without a declaration the return is unknowable.
        "fn" => ctx
            .schema()
            .functions
            .get(path)
            .and_then(|function| function.return_kind.clone())
            .unwrap_or(Kind::Any),
        _ => Kind::Any,
    }
}

/// The type of a decoded JSON value: SurrealDB parses JSON bodies with
/// `json_to_value`, which only produces these kinds.
pub(crate) fn json_value_kind() -> Kind {
    Kind::either(vec![
        Kind::Object,
        Kind::Array(Box::new(Kind::Any), None),
        Kind::String,
        Kind::Number,
        Kind::Bool,
        Kind::Null,
    ])
}

/// The compile-time value of the argument at `index`, when it is
/// statically known (a literal, a composite of literals, or a binding
/// tracing back to one).
pub(crate) fn const_value_arg(
    ctx: &crate::analyzer::context::AnalysisContext<'_>,
    call: &ast::Call,
    index: usize,
) -> Option<surrealdb_types::Value> {
    let arg = call.args.get(index)?;
    let scope = crate::analyzer::expression::infer::InferScope::from_ctx(ctx);
    crate::analyzer::expression::infer::infer_expression_fact(arg, &scope).value
}

/// The closure expression at argument position `index`, when the call site
/// provides one. (Synthetic calls from pure method dispatch have no
/// argument expressions.)
pub(crate) fn closure_arg(call: &ast::Call, index: usize) -> Option<&ast::Closure> {
    match call.args.get(index).map(|arg| &arg.node) {
        Some(ast::Expr::Closure(closure)) => Some(closure),
        _ => None,
    }
}

/// Return-kind lookup for pure inference contexts (projections, nested
/// facts) that have no [`AnalysisContext`]. Function analyzers receive a
/// context so findings can be emitted from them and so value-dependent
/// analyzers can consult the scope; the scope's environment and row table
/// travel into the dispatch.
pub(crate) fn builtin_return_kind_for_call(
    scope: &crate::analyzer::expression::infer::InferScope<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let mut diagnostics = Vec::new();
    let mut ctx = crate::analyzer::context::AnalysisContext::scoped(
        scope.schema,
        scope.source.clone(),
        scope.text,
        &mut diagnostics,
        scope.env.clone(),
        scope.row_table,
    );
    analyze_builtin_function(&mut ctx, call, args)
}

/// [`builtin_return_kind_for_call`] with a synthetic call, for dispatch by
/// name only (method-call sugar like `value.len()`).
pub(crate) fn builtin_return_kind(
    scope: &crate::analyzer::expression::infer::InferScope<'_>,
    path: &str,
    args: &[Kind],
) -> Kind {
    let call = ast::Call {
        path: ast::Spanned::new(
            path.to_string(),
            surrealguard_syntax::span::ByteRange::new(0, 0).expect("empty range is ordered"),
        ),
        args: Vec::new(),
    };
    builtin_return_kind_for_call(scope, &call, args)
}

#[cfg(test)]
mod tests {
    use surrealguard_diagnostics::Finding;
    use surrealguard_syntax::ast;
    use surrealguard_syntax::lower::lower_expr;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use super::analyze_builtin_function;

    use crate::analyzer::context::AnalysisContext;
    use crate::schema::SchemaIndex;

    #[test]
    fn dispatches_documented_function_categories_to_individual_analyzers() {
        let cases = [
            ("RETURN array::len([1]);", "array::len"),
            (
                "RETURN crypto::argon2::generate('pw');",
                "crypto::argon2::generate",
            ),
            ("RETURN duration::from_secs(1);", "duration::from_secs"),
            ("RETURN file::exists('bucket', 'key');", "file::exists"),
            ("RETURN geo::distance((0, 0), (1, 1));", "geo::distance"),
            ("RETURN http::get('https://example.com');", "http::get"),
            ("RETURN math::mean([1, 2]);", "math::mean"),
            ("RETURN meta::id(user:one);", "meta::id"),
            ("RETURN object::keys({ a: 1 });", "object::keys"),
            (
                "RETURN parse::url::domain('https://surrealdb.com');",
                "parse::url::domain",
            ),
            ("RETURN rand::uuid();", "rand::uuid"),
            ("RETURN record::exists(user:one);", "record::exists"),
            ("RETURN search::score(1);", "search::score"),
            ("RETURN sequence::nextval('invoice');", "sequence::nextval"),
            ("RETURN session::db();", "session::db"),
            ("RETURN set::len({1, 2});", "set::len"),
            ("RETURN string::split('a b', ' ');", "string::split"),
            ("RETURN time::now();", "time::now"),
            ("RETURN type::is::record(user:one);", "type::is_record"),
            ("RETURN value::diff({ a: 1 }, { a: 2 });", "value::diff"),
            ("RETURN vector::dot([1], [2]);", "vector::dot"),
        ];

        for (query, expected) in cases {
            assert_function_dispatch(query, expected);
        }
    }

    fn assert_function_dispatch(query: &str, expected: &'static str) {
        // This proves lowering's path normalization + namespace/function
        // routing reach the right analyzer for every documented category
        // without panicking — it intentionally does not assert a specific
        // `Kind`; each function's signature is covered by its own file's
        // tests.
        let parsed = parse_source(SourceId::new(format!("query:{expected}")), query)
            .expect("query should parse");
        let node = first_node_of_kind(parsed.tree().root_node(), "FunctionCall")
            .unwrap_or_else(|| panic!("no function call node found for {query}"));
        let lowered = lower_expr(node, parsed.text());
        let ast::Expr::Call(call) = &lowered.node else {
            panic!("expected call lowering for {query}, got {:?}", lowered.node);
        };
        assert_eq!(call.path.node, expected, "normalized path for {query}");

        let schema = SchemaIndex::default();
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            parsed.source_id().clone(),
            parsed.text(),
            &mut diagnostics,
        );

        let _ = analyze_builtin_function(&mut ctx, call, &[]);
    }

    fn first_node_of_kind<'tree>(
        node: tree_sitter::Node<'tree>,
        kind: &str,
    ) -> Option<tree_sitter::Node<'tree>> {
        if node.kind() == kind {
            return Some(node);
        }

        let mut cursor = node.walk();
        let found = node
            .children(&mut cursor)
            .find_map(|child| first_node_of_kind(child, kind));
        found
    }
}
