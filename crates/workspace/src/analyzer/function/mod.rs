//! Built-in SurrealQL function analyzers.
//!
//! `analyze_builtin_function` is the namespace entrypoint. It extracts the
//! function path from the source text, routes to a category `mod.rs`, and the
//! category fans out to one analyzer function per documented built-in function.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;
use surrealguard_syntax::span::SourceSpan;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::contract::{Contract, Position};
use crate::analyzer::facts::Bindings;

pub mod api;
pub mod array;
pub mod bytes;
pub mod count;
pub mod crypto;
pub mod duration;
pub mod encoding;
pub mod eval;
pub mod file;
pub mod geo;
pub mod http;
pub mod math;
pub mod meta;
pub mod object;
pub mod parse;
pub mod rand;
pub mod record;
pub mod schema;
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

pub(crate) fn analyze_builtin_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    // An empty call path is not a builtin lookup: it is a param-invocation
    // like `$priority($cur.source)` — calling a variable that holds a
    // closure. There is no function name to resolve, so yield `Any` rather
    // than emitting a spurious "unknown function" (5001).
    if call.path.node.is_empty() {
        return Kind::Any;
    }

    // `call.path` is pre-normalized by lowering (`type::is::record` ->
    // `type::is_record`); custom `fn::*` functions fall through to `Any`.
    let path = call.path.node.as_str();

    // Against the WRITTEN spelling: `type::is::record` and `type::is_record`
    // are the same `path` by this point, and only one of them parses.
    if let Some((retired, replacement)) = RETIRED_FUNCTIONS
        .iter()
        .find(|(retired, _)| *retired == call.written)
    {
        return retired_function(ctx, call, retired, replacement);
    }

    match path.split("::").next().unwrap_or_default() {
        "api" => api::analyze_api_function(ctx, call, path, args),
        "array" => array::analyze_array_function(ctx, call, path, args),
        "bytes" => bytes::analyze_bytes_function(ctx, call, path, args),
        "count" => count::analyze_count_function(ctx, call, path, args),
        "crypto" => crypto::analyze_crypto_function(ctx, call, path, args),
        "duration" => duration::analyze_duration_function(ctx, call, path, args),
        "encoding" => encoding::analyze_encoding_function(ctx, call, path, args),
        "eval" => eval::analyze_eval_function(ctx, call, path, args),
        "file" => file::analyze_file_function(ctx, call, path, args),
        "geo" => geo::analyze_geo_function(ctx, call, path, args),
        "http" => http::analyze_http_function(ctx, call, path, args),
        "math" => math::analyze_math_function(ctx, call, path, args),
        "meta" => meta::analyze_meta_function(ctx, call, path, args),
        "object" => object::analyze_object_function(ctx, call, path, args),
        "parse" => parse::analyze_parse_function(ctx, call, path, args),
        "rand" => rand::analyze_rand_function(ctx, call, path, args),
        "record" => record::analyze_record_function(ctx, call, path, args),
        "schema" => schema::analyze_schema_function(ctx, call, path, args),
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
        // User-defined functions carry their declared signature on the
        // schema; calls check against it (5002) like any builtin.
        "fn" => match ctx.schema().functions.get(path) {
            Some(function) => {
                let function = function.clone();
                check_custom_call(ctx, call, &function, args);
                // The declared `-> T` is authoritative; when the definition
                // omits it, fall back to the kind inferred from the body so
                // untyped `fn::` helpers still propagate a real type.
                function
                    .return_kind
                    .clone()
                    .or_else(|| function.inferred_return.clone())
                    .unwrap_or(Kind::Any)
            }
            None => {
                if !is_synthetic(call) {
                    let span = SourceSpan::new(ctx.source().clone(), call.path.span);
                    let mut finding = surrealguard_diagnostics::catalog::finding(
                        span,
                        5001,
                        format!("`{path}` is not a defined function"),
                    );
                    match crate::suggest::closest(
                        path,
                        ctx.schema().functions.keys().map(String::as_str),
                    ) {
                        Some(nearest) => {
                            finding = finding.with_help(format!("did you mean `{nearest}`?"));
                        }
                        None => {
                            finding = finding.with_help(format!(
                                "no `DEFINE FUNCTION {path}` exists in the workspace"
                            ));
                        }
                    }
                    ctx.emit(finding);
                }
                Kind::Any
            }
        },
        _ => unknown_function(ctx, call),
    }
}

/// Function names SurrealDB has **removed**, each paired with the spelling that
/// replaced it.
///
/// These are not merely unregistered names. SurrealDB 3.2.3 refuses to *parse*
/// a call to one, so the query never reaches execution at all — verified live:
///
/// ```text
/// RETURN type::thing('person', 'ada');
///   --< Parse error: Invalid function/constant path, did you maybe mean `type::record`
/// RETURN type::record('person', 'ada');   -> ["person:ada"]
/// ```
///
/// SurrealGuard analyzes for the latest release, so a retired name is simply an
/// unknown name (5001) — the same contract `string::endsWith` already reports
/// under — carrying the engine's own suggestion.
///
/// Matched against the path **as written**, not the canonical one. Lowering
/// rewrites `::is::` to `::is_`, which is precisely what made the retired
/// spelling indistinguishable from the live one here; `call.written` keeps
/// them apart, so the whole colon-form family can be listed.
///
/// Every row was probed against 3.2.3 (`RETURN <name>()`, reading the parse
/// error apart from the arity error) and every suggestion is the engine's own
/// `did you maybe mean`, except where it offers none.
const RETIRED_FUNCTIONS: &[(&str, &str)] = &[
    // Renamed or removed outright, each verified against 3.2.3. The two with
    // no replacement have none inside their namespace: `not::not(x)` is the
    // `!x` operator, and reference traversal is the `<~` idiom — the engine
    // offers no `did you maybe mean` for either.
    ("count::count", "count"),
    ("meta::table", "meta::tb"),
    ("not::not", ""),
    ("rand::guid", "rand::id"),
    ("record::refs", ""),
    ("sequence::next", "sequence::nextval"),
    ("session::sc", "session::ac"),
    ("session::sd", "session::rd"),
    ("type::thing", "type::record"),

    // The `::from::` families, retired wholesale in 3.x.
    ("duration::from::days", "duration::from_days"),
    ("duration::from::hours", "duration::from_hours"),
    ("duration::from::micros", "duration::from_micros"),
    ("duration::from::millis", "duration::from_millis"),
    ("duration::from::mins", "duration::from_mins"),
    ("duration::from::nanos", "duration::from_nanos"),
    ("duration::from::secs", "duration::from_secs"),
    ("duration::from::weeks", "duration::from_weeks"),
    ("time::from::micros", "time::from_micros"),
    ("time::from::millis", "time::from_millis"),
    ("time::from::nanos", "time::from_nanos"),
    ("time::from::secs", "time::from_secs"),
    ("time::from::unix", "time::from_unix"),

    // Every `::is::` spelling, likewise. One row per predicate the registry
    // knows in its underscore form, so a row exists only where the
    // replacement it names really does.
    ("array::is::empty", "array::is_empty"),
    ("geo::is::valid", "geo::is_valid"),
    ("object::is::empty", "object::is_empty"),
    ("record::is::edge", "record::is_edge"),
    ("set::is::empty", "set::is_empty"),
    ("string::is::alpha", "string::is_alpha"),
    ("string::is::alphanum", "string::is_alphanum"),
    ("string::is::ascii", "string::is_ascii"),
    ("string::is::datetime", "string::is_datetime"),
    ("string::is::domain", "string::is_domain"),
    ("string::is::email", "string::is_email"),
    ("string::is::hexadecimal", "string::is_hexadecimal"),
    ("string::is::ip", "string::is_ip"),
    ("string::is::latitude", "string::is_latitude"),
    ("string::is::longitude", "string::is_longitude"),
    ("string::is::numeric", "string::is_numeric"),
    ("string::is::record", "string::is_record"),
    ("string::is::semver", "string::is_semver"),
    ("string::is::ulid", "string::is_ulid"),
    ("string::is::url", "string::is_url"),
    ("string::is::uuid", "string::is_uuid"),
    ("time::is::leap_year", "time::is_leap_year"),
    ("type::is::array", "type::is_array"),
    ("type::is::bool", "type::is_bool"),
    ("type::is::bytes", "type::is_bytes"),
    ("type::is::collection", "type::is_collection"),
    ("type::is::datetime", "type::is_datetime"),
    ("type::is::decimal", "type::is_decimal"),
    ("type::is::duration", "type::is_duration"),
    ("type::is::float", "type::is_float"),
    ("type::is::geometry", "type::is_geometry"),
    ("type::is::int", "type::is_int"),
    ("type::is::line", "type::is_line"),
    ("type::is::multiline", "type::is_multiline"),
    ("type::is::multipoint", "type::is_multipoint"),
    ("type::is::multipolygon", "type::is_multipolygon"),
    ("type::is::none", "type::is_none"),
    ("type::is::null", "type::is_null"),
    ("type::is::number", "type::is_number"),
    ("type::is::object", "type::is_object"),
    ("type::is::point", "type::is_point"),
    ("type::is::polygon", "type::is_polygon"),
    ("type::is::range", "type::is_range"),
    ("type::is::record", "type::is_record"),
    ("type::is::set", "type::is_set"),
    ("type::is::string", "type::is_string"),
    ("type::is::uuid", "type::is_uuid"),
];

/// Reports a call to a removed function (5001) and yields `Any`, exactly as an
/// unknown name does — the call resolves to nothing on the engine either.
fn retired_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    retired: &str,
    replacement: &str,
) -> Kind {
    if !is_synthetic(call) {
        let span = SourceSpan::new(ctx.source().clone(), call.path.span);
        // The written spelling, so the message quotes what the author typed
        // rather than the canonical form lowering rewrote it into.
        let finding = surrealguard_diagnostics::catalog::finding(
            span,
            5001,
            format!("`{retired}` was removed from SurrealQL; it is not a known function"),
        );
        // Two of these have no replacement inside their namespace, and the
        // engine offers none either. A "use `` instead" would be worse than
        // the bare fact.
        ctx.emit(if replacement.is_empty() {
            finding.with_help("SurrealDB 3.2.3 rejects this call while parsing, so the query never runs")
        } else {
            finding.with_help(format!("use `{replacement}` instead"))
        });
    }
    Kind::Any
}

/// Fallthrough for a call that resolved to no builtin: emits 5001 unless
/// the call is synthetic (the receiver-kind method probe intentionally
/// tries names that may not exist).
pub(crate) fn unknown_function(ctx: &mut AnalysisContext<'_>, call: &ast::Call) -> Kind {
    if !is_synthetic(call) {
        let span = SourceSpan::new(ctx.source().clone(), call.path.span);
        let mut finding = surrealguard_diagnostics::catalog::finding(
            span,
            5001,
            format!("`{}` is not a known function", call.path.node),
        );
        if let Some(nearest) = crate::suggest::closest(
            call.path.node.as_str(),
            ctx.schema().functions.keys().map(String::as_str),
        ) {
            finding = finding.with_help(format!("did you mean `{nearest}`?"));
        }
        ctx.emit(finding);
    }
    Kind::Any
}

fn is_synthetic(call: &ast::Call) -> bool {
    call.path.span.start() == call.path.span.end()
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

/// A call value carrying a path *and* the method's argument expressions — for
/// dispatch by name from method-call sugar (`$rows.map(|$o| $o.name)`).
///
/// The receiver is argument 0 of the family function (`x.len()` is
/// `string::len(x)`), but it is a *prefix of the idiom*, not an argument node,
/// so a `Partial` placeholder holds position 0 and the real arguments line up
/// with the indices the analyzers read. That alignment is the whole point:
/// without it `closure_arg(call, 1)` found nothing and every closure-taking
/// built-in fell back to `Kind::Any`.
///
/// The placeholder infers to no kind and no value, so a const-value read of
/// argument 0 yields `None` — exactly what it yielded when synthetic calls
/// carried no arguments at all. The path span stays empty, so the call is still
/// [`is_synthetic`] and the signature table stays inference-only on it.
pub(crate) fn synthetic_method_call(path: &str, args: &[ast::Spanned<ast::Expr>]) -> ast::Call {
    let empty = surrealguard_syntax::span::ByteRange::new(0, 0).expect("empty range is ordered");
    let receiver = ast::Spanned::new(
        ast::Expr::Partial(ast::PartialNode {
            span: empty,
            cst_kind: "MethodReceiver".to_string(),
        }),
        empty,
    );
    let mut call = synthetic_call(path);
    call.args = std::iter::once(receiver)
        .chain(args.iter().cloned())
        .collect();
    call
}

/// A call value carrying only a path — for dispatch by name, where no argument
/// expressions exist.
pub(crate) fn synthetic_call(path: &str) -> ast::Call {
    ast::Call {
        path: ast::Spanned::new(
            path.to_string(),
            surrealguard_syntax::span::ByteRange::new(0, 0).expect("empty range is ordered"),
        ),
        // Nothing wrote a synthetic call, so its written spelling is its
        // canonical one — and never a retired form, which is what keeps a
        // method-call desugaring from reporting a spelling the author did not
        // use.
        written: path.to_string(),
        args: Vec::new(),
    }
}

/// The compile-time value of the argument at `index`, when it is
/// statically known (a literal, a composite of literals, or a binding
/// tracing back to one).
pub(crate) fn const_value_arg(
    ctx: &mut crate::analyzer::context::AnalysisContext<'_>,
    call: &ast::Call,
    index: usize,
) -> Option<surrealdb_types::Value> {
    let arg = call.args.get(index)?;
    crate::analyzer::expression::infer::infer_expression_fact(arg, ctx).value
}

/// `fn::` calls check against the DEFINE FUNCTION signature: argument
/// count and, where the params declare kinds, per-argument kinds (5002).
fn check_custom_call(
    ctx: &mut crate::analyzer::context::AnalysisContext<'_>,
    call: &ast::Call,
    function: &crate::schema::FunctionDef,
    args: &[Kind],
) {
    if call.path.span.start() == call.path.span.end() {
        return;
    }
    if args.len() != function.args.len() {
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), call.path.span);
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                5002,
                format!(
                    "`{}` takes {} {}, but this call passes {}",
                    call.path.node,
                    function.args.len(),
                    if function.args.len() == 1 {
                        "argument"
                    } else {
                        "arguments"
                    },
                    args.len()
                ),
            )
            .with_related(
                function.name_span.clone(),
                format!("`{}` is defined here", function.name),
            ),
        );
        return;
    }
    for (index, (param, kind)) in function.args.iter().zip(args).enumerate() {
        let Some(expected) = &param.kind else {
            continue;
        };
        if let Some(arg_expr) = call.args.get(index) {
            if let ast::Expr::Param(param) = &arg_expr.node {
                if *kind == Kind::Any {
                    let span = surrealguard_syntax::span::SourceSpan::new(
                        ctx.source().clone(),
                        arg_expr.span,
                    );
                    ctx.constrain_param(param, span, expected.clone(), None);
                    continue;
                }
            }
        }
        // A declared parameter is a contract like any other, so a written
        // constant is compared as the literal it is: `fn::take('green')`
        // against `$a: 'red' | 'blue'` is a provable mismatch, where the kind
        // inference widened it to (`string`) never could be.
        let folded = call.args.get(index).and_then(|arg| {
            crate::analyzer::contract::term_kind(&crate::analyzer::facts::eval(
                &arg.node,
                Bindings::NONE,
            ))
        });
        let actual = folded.unwrap_or_else(|| kind.clone());
        let contract = Contract::new(Position::FunctionArg, expected.clone());
        if !contract.decide(&actual).is_violation() {
            continue;
        }
        let Some(arg_expr) = call.args.get(index) else {
            continue;
        };
        let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), arg_expr.span);
        ctx.emit(
            surrealguard_diagnostics::catalog::finding(
                span,
                contract.code(),
                format!(
                    "argument {} to `{}` is a `{}`, but `${}` is declared `{}`",
                    index + 1,
                    call.path.node,
                    crate::render::render_offending(&actual, Some(expected)),
                    param.name,
                    crate::render_kind(expected),
                ),
            )
            .with_help(format!(
                "pass a `{}`, or widen `${}` to accept `{}`",
                crate::render_kind(expected),
                param.name,
                crate::render::render_offending(kind, Some(expected)),
            ))
            .with_related(
                function.name_span.clone(),
                format!("`{}` is defined here", function.name),
            ),
        );
    }
}

/// A consumer invokes its closure with a fixed argument list; declaring
/// more parameters than it passes leaves the extras unbound (5004).
pub(crate) fn check_closure_arity(
    ctx: &mut crate::analyzer::context::AnalysisContext<'_>,
    call: &ast::Call,
    closure: &ast::Closure,
    provided: usize,
) {
    if closure.params.len() <= provided {
        return;
    }
    let Some((name, _)) = closure.params.get(provided) else {
        return;
    };
    let span = surrealguard_syntax::span::SourceSpan::new(ctx.source().clone(), name.span);
    ctx.emit(surrealguard_diagnostics::catalog::finding(
        span,
        5002,
        format!(
            "`{}` calls its closure with {provided} {}; `${}` is never bound",
            call.path.node,
            if provided == 1 {
                "argument"
            } else {
                "arguments"
            },
            name.node,
        ),
    ));
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

#[cfg(test)]
mod tests {
    use surrealguard_diagnostics::Finding;
    use surrealguard_syntax::ast;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use super::{analyze_builtin_function, synthetic_call};
    use surrealdb_types::Kind;

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
        let lowered = surrealguard_syntax::lower::lower_first_expr(&parsed, "FunctionCall")
            .unwrap_or_else(|| panic!("no function call node found for {query}"));
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

    /// Sweeps every registered builtin through the dispatcher: each
    /// dispatch arm (collected from the family dispatchers' own source)
    /// must analyze a synthetic call at several arities without panicking.
    /// This executes every signature-literal leaf file.
    #[test]
    fn every_registered_builtin_analyzes_synthetic_calls() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/analyzer/function");
        let mut names = std::collections::BTreeSet::new();
        collect_dispatch_arms(&root, &mut names);
        assert!(
            names.len() > 250,
            "dispatch-arm scan looks broken: found only {} names",
            names.len()
        );

        let arg_shapes: Vec<Vec<Kind>> = vec![
            vec![],
            vec![Kind::Any],
            vec![Kind::Any, Kind::Any],
            vec![Kind::Array(Box::new(Kind::Any), None), Kind::Any, Kind::Any],
        ];

        let schema = SchemaIndex::default();
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            surrealguard_syntax::source::SourceId::new("sweep"),
            "",
            &mut diagnostics,
        );
        for name in &names {
            let call = synthetic_call(name);
            for args in &arg_shapes {
                let _ = analyze_builtin_function(&mut ctx, &call, args);
            }
        }
    }

    /// The response kind of `query` analyzed as a standalone source.
    fn response_kind_of(query: &str) -> Option<Kind> {
        let mut workspace = crate::analysis::Workspace::default();
        crate::analysis::analyze_query(&mut workspace, query).response_kind
    }

    /// The findings raised for `query` analyzed as a standalone source.
    fn diagnostics_of(query: &str) -> Vec<Finding> {
        let mut workspace = crate::analysis::Workspace::default();
        crate::analysis::analyze_query(&mut workspace, query).diagnostics
    }

    /// SX-4: whole builtin families (three- and four-segment names, plus
    /// bare `rand()`) were absent from the registry, so real SurrealQL got
    /// a false `E5001 unknown function` — which exits `check` non-zero and
    /// aborts `generate` for the whole workspace.
    #[test]
    fn newly_registered_builtin_families_resolve_with_their_return_kinds() {
        let cases: Vec<(&str, Kind)> = vec![
            (
                "RETURN vector::distance::euclidean([1.0], [2.0]);",
                Kind::Number,
            ),
            (
                "RETURN vector::distance::minkowski([1.0], [2.0], 3);",
                Kind::Number,
            ),
            (
                "RETURN vector::similarity::cosine([1.0], [2.0]);",
                Kind::Number,
            ),
            (
                "RETURN array::sort::asc([3, 1]);",
                Kind::Array(Box::new(Kind::Int), Some(2)),
            ),
            (
                "RETURN array::sort::desc([3, 1]);",
                Kind::Array(Box::new(Kind::Int), Some(2)),
            ),
            ("RETURN rand();", Kind::Float),
            ("RETURN rand::uuid::v4();", Kind::Uuid),
            ("RETURN rand::uuid::v7();", Kind::Uuid),
            ("RETURN rand::duration(1s, 2s);", Kind::Duration),
            ("RETURN string::semver::major('1.2.3');", Kind::Int),
            ("RETURN string::semver::inc::patch('1.2.3');", Kind::String),
            (
                "RETURN string::semver::set::minor('1.2.3', 4);",
                Kind::String,
            ),
            ("RETURN string::distance::levenshtein('a', 'b');", Kind::Int),
            (
                "RETURN string::distance::normalized_levenshtein('a', 'b');",
                Kind::Float,
            ),
            (
                "RETURN string::similarity::jaro_winkler('a', 'b');",
                Kind::Float,
            ),
            ("RETURN string::html::encode('<b>');", Kind::String),
            ("RETURN geo::hash::encode((0, 0), 8);", Kind::String),
            ("RETURN schema::table::exists('user');", Kind::Bool),
            // The underscore spellings, which are the only ones 3.2.3 parses.
            // These rows said `duration::from::days` / `time::from::unix` and
            // asserted they analyze CLEANLY — a 2.x assumption. Both are parse
            // errors on the engine now, and both are reported as 5001; the
            // retired-spelling test below is where they are pinned.
            ("RETURN duration::from_days(3);", Kind::Duration),
            ("RETURN time::from_unix(1);", Kind::Datetime),
            ("RETURN type::is_set([1]);", Kind::Bool),
            ("RETURN array::index_of([1, 2], 2);", Kind::Int),
        ];

        for (query, expected) in cases {
            assert_eq!(
                diagnostics_of(query),
                Vec::new(),
                "`{query}` must analyze cleanly"
            );
            assert_eq!(response_kind_of(query), Some(expected), "kind of `{query}`");
        }
    }

    /// A name the engine no longer parses is not a name we can type-check.
    /// SurrealDB 3.2.3 answers `RETURN type::thing('person', 'ada')` with
    /// "Parse error: Invalid function/constant path, did you maybe mean
    /// `type::record`" — the query never runs, so silently inferring a
    /// `record<person>` for it was assurance about a query that cannot execute.
    #[test]
    fn a_removed_function_is_reported_with_the_spelling_that_replaced_it() {
        let findings = diagnostics_of("RETURN type::thing('person', 'ada');");
        let finding = findings
            .iter()
            .find(|finding| finding.code().number() == 5001)
            .expect("expected 5001 for a removed function");
        assert!(
            finding.message().contains("`type::thing` was removed"),
            "unexpected message: {}",
            finding.message()
        );
        assert!(
            finding
                .help()
                .iter()
                .any(|help| help.message.contains("`type::record`")),
            "the finding must name the replacement: {:?}",
            finding.help()
        );

        // The replacement itself stays clean, and keeps its inference.
        assert_eq!(
            diagnostics_of("RETURN type::record('person', 'ada');"),
            Vec::new()
        );
        assert_eq!(
            response_kind_of("RETURN type::record('person', 'ada');"),
            Some(Kind::Record(vec!["person".into()]))
        );
    }

    /// Every row of the table, and the live spelling each one names.
    ///
    /// The table is data, and data rots quietly: a row whose retired name the
    /// analyzer stopped routing through this check, or one whose replacement
    /// is itself not a function, would both go unnoticed. This walks the whole
    /// thing — a retired name must report 5001, and the name it points at must
    /// not.
    ///
    /// Both halves matter. The first is the contract. The second is the guard
    /// against the failure this fix nearly shipped: the completion catalog
    /// listed only the `type::is::x` spelling of that family, so retiring the
    /// colon form silently removed the underscore form as well and
    /// `age.is_none()` stopped resolving.
    #[test]
    fn every_retired_spelling_reports_and_every_replacement_it_names_does_not() {
        for (retired, replacement) in super::RETIRED_FUNCTIONS {
            let findings = diagnostics_of(&format!("RETURN {retired}();"));
            assert!(
                findings
                    .iter()
                    .any(|finding| finding.code().number() == 5001
                        && finding.message().contains(&format!("`{retired}` was removed"))),
                "`{retired}` is retired but was not reported: {findings:?}"
            );
            if replacement.is_empty() {
                continue;
            }
            // Called with no arguments, so an arity finding (5002) is expected
            // and uninteresting; what must not appear is 5001, which would mean
            // we are pointing the author at a name we do not know either.
            let findings = diagnostics_of(&format!("RETURN {replacement}();"));
            assert!(
                !findings
                    .iter()
                    .any(|finding| finding.code().number() == 5001),
                "`{retired}`'s replacement `{replacement}` is not a known function: {findings:?}"
            );
        }
    }

    /// The colon spellings are the ones lowering used to erase: it rewrote
    /// `::is::` to `::is_` before any analyzer saw a path, so the dead spelling
    /// and the live one were the same string by the time anything could tell
    /// them apart. `call.written` is what keeps them distinct.
    #[test]
    fn the_colon_spellings_report_while_their_underscore_forms_stay_clean() {
        for (retired, live) in [
            ("type::is::string", "type::is_string"),
            ("string::is::email", "string::is_email"),
            ("array::is::empty", "array::is_empty"),
            ("duration::from::days", "duration::from_days"),
            ("time::from::unix", "time::from_unix"),
        ] {
            assert!(
                diagnostics_of(&format!("RETURN {retired}('x');"))
                    .iter()
                    .any(|finding| finding.code().number() == 5001),
                "`{retired}` must report 5001"
            );
            assert!(
                !diagnostics_of(&format!("RETURN {live}('x');"))
                    .iter()
                    .any(|finding| finding.code().number() == 5001),
                "`{live}` must stay a known function"
            );
        }
    }

    #[test]
    fn an_unregistered_name_in_a_registered_family_still_raises_unknown_function() {
        // The must-still-fire boundary: adding the real `vector::distance::*`
        // members must not turn the family into a wildcard.
        for query in [
            "RETURN vector::distance::bogus([1.0], [2.0]);",
            "RETURN string::semver::bogus('1.2.3');",
            "RETURN array::sort::sideways([1]);",
            "RETURN rand::uuid::v9();",
            "RETURN schema::table::bogus('user');",
        ] {
            let codes: Vec<String> = diagnostics_of(query)
                .iter()
                .map(|finding| finding.code().to_string())
                .collect();
            assert!(
                codes.iter().any(|code| code == "E5001"),
                "`{query}` should still be unknown, got {codes:?}"
            );
        }
    }

    #[test]
    fn untyped_udf_call_resolves_to_its_inferred_body_kind() {
        // No `-> T`: the caller sees the body's inferred `int`, not `Any`.
        let query = "DEFINE FUNCTION fn::double($x: int) { RETURN $x * 2; };\n\
                     RETURN fn::double(3);";
        assert_eq!(response_kind_of(query), Some(Kind::Int));
        assert_eq!(diagnostics_of(query), Vec::new());
    }

    #[test]
    fn declared_return_wins_over_the_inferred_body_kind() {
        // The declared `-> string` is authoritative at the call site.
        let query = "DEFINE FUNCTION fn::greet($n: string) -> string { RETURN $n; };\n\
                     RETURN fn::greet('hi');";
        assert_eq!(response_kind_of(query), Some(Kind::String));
        assert_eq!(diagnostics_of(query), Vec::new());
    }

    #[test]
    fn genuinely_untyped_udf_call_stays_any() {
        // The body returns an untyped param, so the call is honestly `Any`.
        let query = "DEFINE FUNCTION fn::opaque($x: any) { RETURN $x; };\n\
                     RETURN fn::opaque(3);";
        assert_eq!(response_kind_of(query), Some(Kind::Any));
        assert_eq!(diagnostics_of(query), Vec::new());
    }

    #[test]
    fn udf_calling_another_udf_resolves_or_safely_falls_back() {
        // `fn::wrap` delegates to `fn::base`; the call resolves through the
        // callee's inferred return, or safely degrades to `Any` — never wrong.
        let query = "DEFINE FUNCTION fn::base($x: int) { RETURN $x * 2; };\n\
                     DEFINE FUNCTION fn::wrap($y: int) { RETURN fn::base($y); };\n\
                     RETURN fn::wrap(3);";
        assert!(
            matches!(response_kind_of(query), Some(Kind::Int) | Some(Kind::Any)),
            "cross-udf call must resolve to int or fall back to any, got {:?}",
            response_kind_of(query),
        );
        assert_eq!(diagnostics_of(query), Vec::new());
    }

    fn collect_dispatch_arms(
        dir: &std::path::Path,
        names: &mut std::collections::BTreeSet<String>,
    ) {
        for entry in std::fs::read_dir(dir).expect("function tree readable") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                collect_dispatch_arms(&path, names);
            } else if path.file_name().is_some_and(|name| name == "mod.rs") {
                let text = std::fs::read_to_string(&path).expect("dispatcher readable");
                for line in text.lines() {
                    let Some(rest) = line.trim().strip_prefix('"') else {
                        continue;
                    };
                    let Some((name, tail)) = rest.split_once('"') else {
                        continue;
                    };
                    if tail.trim_start().starts_with("=>") && name.contains("::") {
                        names.insert(name.to_string());
                    }
                }
            }
        }
    }
}
