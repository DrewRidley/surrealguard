//! Built-in SurrealQL function analyzers.
//!
//! Every built-in is one [`BuiltinEntry`] in its family's `CATALOG`
//! (`<family>/mod.rs`): the name as dispatched, a one-line doc, the leaf
//! file's declared [`Signature`], and the analyzer that infers a call's
//! kind. [`builtin_catalog`] merges the families into one sorted table, and
//! `analyze_builtin_function` — the namespace entrypoint — dispatches by
//! looking a call's path up in it. The table *is* the dispatch, so the set
//! of names the analyzer resolves and the set completion offers cannot
//! drift: both read the same rows.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;
use surrealguard_syntax::span::SourceSpan;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::contract::{Contract, Position};
use crate::analyzer::facts::Bindings;
use crate::analyzer::version::FunctionVersion;
use signature::Signature;

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
pub mod not;
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

/// One built-in function the analyzer resolves: a row of a family's
/// `CATALOG`, and of [`builtin_catalog`].
#[derive(Clone, Copy)]
pub struct BuiltinEntry {
    /// The call path as the analyzer dispatches it (`string::len`,
    /// `type::is_record`, bare `count`).
    pub name: &'static str,
    /// A one-line description for editors. Empty for a spelling the analyzer
    /// accepts but SurrealDB does not document (`count::count`); completion
    /// offers only documented entries.
    pub doc: &'static str,
    /// The leaf file's declared signature — shared by its analyzer and by
    /// everything that renders the function.
    signature: fn() -> Signature,
    /// The analyzer: checks the call where the signature allows, and infers
    /// its kind.
    analyze: fn(&mut AnalysisContext<'_>, &ast::Call, &[Kind]) -> Kind,
}

impl BuiltinEntry {
    /// A catalog row. `const` so the family tables are plain statics.
    pub(crate) const fn new(
        name: &'static str,
        doc: &'static str,
        signature: fn() -> Signature,
        analyze: fn(&mut AnalysisContext<'_>, &ast::Call, &[Kind]) -> Kind,
    ) -> Self {
        Self {
            name,
            doc,
            signature,
            analyze,
        }
    }

    /// The leading namespace (`string`, `math`; `count` for the bare
    /// `count`), which is the family a method call on a typed receiver
    /// dispatches to.
    pub fn family(&self) -> &'static str {
        match self.name.split_once("::") {
            Some((head, _)) => head,
            None => self.name,
        }
    }

    /// The declared signature: arity, per-argument expectations, and how the
    /// return kind derives from the arguments.
    pub(crate) fn signature(&self) -> Signature {
        (self.signature)()
    }

    /// The name's version facts, when a release added or removed it.
    pub fn version(&self) -> Option<&'static FunctionVersion> {
        crate::analyzer::version::function_version(self.name)
    }

    /// Whether the name exists in current SurrealDB — `false` for a spelling
    /// a release removed (`duration::from::days`, renamed in 3.0), which the
    /// analyzer still dispatches so the 8001 rename hint has a signature to
    /// stand on, but which no editor should offer.
    pub fn is_current(&self) -> bool {
        self.version()
            .is_none_or(|version| version.removed.is_none())
    }

    /// Whether SurrealDB documents this spelling; see [`BuiltinEntry::doc`].
    pub fn is_documented(&self) -> bool {
        !self.doc.is_empty()
    }
}

/// Every built-in the analyzer resolves, sorted by name: the 29 family
/// `CATALOG` tables merged.
pub fn builtin_catalog() -> &'static [BuiltinEntry] {
    static CATALOG: std::sync::OnceLock<Vec<BuiltinEntry>> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut entries: Vec<BuiltinEntry> = FAMILIES
            .iter()
            .flat_map(|family| family.iter())
            .copied()
            .collect();
        entries.sort_by(|a, b| a.name.cmp(b.name));
        entries
    })
}

/// The family tables, one per `pub mod` above.
const FAMILIES: &[&[BuiltinEntry]] = &[
    api::CATALOG,
    array::CATALOG,
    bytes::CATALOG,
    count::CATALOG,
    crypto::CATALOG,
    duration::CATALOG,
    encoding::CATALOG,
    eval::CATALOG,
    file::CATALOG,
    geo::CATALOG,
    http::CATALOG,
    math::CATALOG,
    meta::CATALOG,
    not::CATALOG,
    object::CATALOG,
    parse::CATALOG,
    rand::CATALOG,
    record::CATALOG,
    schema::CATALOG,
    search::CATALOG,
    sequence::CATALOG,
    session::CATALOG,
    set::CATALOG,
    sleep::CATALOG,
    string::CATALOG,
    time::CATALOG,
    type_::CATALOG,
    value::CATALOG,
    vector::CATALOG,
];

/// The catalog entry dispatched for `path` — the exact name, as lowering
/// produces it (`type::is::record` is already `type::is_record` here).
pub fn builtin(path: &str) -> Option<&'static BuiltinEntry> {
    let catalog = builtin_catalog();
    catalog
        .binary_search_by(|entry| entry.name.cmp(path))
        .ok()
        .map(|index| &catalog[index])
}

/// Whether `path` names a built-in the analyzer resolves.
///
/// This is the *existence* oracle, deliberately separate from what a call
/// evaluates to. Several built-ins return an honest `Kind::Any` — `record::id`
/// (a record id is genuinely one of many shapes), `array::at` on an
/// `array<any>` — so "the analyzer produced `any`" cannot stand in for "no such
/// function": doing that turned `$r.id()` into a false `E5001 has no method`.
///
/// Names are canonicalized by replacing `::` with `_`, which folds the two
/// spellings of the `type::is::x` family (`type::is::record` as written,
/// `type::is_record` as lowered and dispatched) onto one key.
pub(crate) fn is_builtin(path: &str) -> bool {
    static NAMES: std::sync::OnceLock<std::collections::BTreeSet<String>> =
        std::sync::OnceLock::new();
    NAMES
        .get_or_init(|| {
            builtin_catalog()
                .iter()
                .map(|entry| entry.name.replace("::", "_"))
                .collect()
        })
        .contains(&path.replace("::", "_"))
}

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
    let mut path = call.path.node.as_str();

    // Does the configured target release have this function (8001)? The
    // question is asked of the name *as written*: lowering has already
    // folded `type::is::record` into `type::is_record`, and which of the two
    // a target accepts is exactly the fact at stake. A removed spelling of a
    // function that still exists is analyzed under its current name — the
    // rename changed nothing about the signature.
    if let Some(target) = ctx.target_version() {
        if !is_synthetic(call) {
            let written = ctx
                .source_text()
                .get(call.path.span.start() as usize..call.path.span.end() as usize)
                .map_or(path, str::trim);
            if let Some(mismatch) = crate::analyzer::version::check_function(target, written) {
                let span = SourceSpan::new(ctx.source().clone(), call.path.span);
                ctx.emit(
                    surrealguard_diagnostics::catalog::finding(span, 8001, mismatch.message)
                        .with_help(mismatch.help),
                );
                if let Some(current) = mismatch.dispatch_as {
                    path = current;
                }
            }
        }
    }

    if let Some(entry) = builtin(path) {
        return (entry.analyze)(ctx, call, args);
    }
    if path.split("::").next() == Some("fn") {
        // User-defined functions carry their declared signature on the
        // schema; calls check against it (5002) like any builtin.
        match ctx.schema().functions.get(path) {
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
        }
    } else {
        unknown_function(ctx, call)
    }
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
        // A name that stopped existing because it was renamed is unknown for
        // a reason worth stating, whatever the target (or lack of one).
        if let Some(hint) = crate::analyzer::version::rename_hint(call.path.node.as_str()) {
            finding = finding.with_help(hint);
        } else if let Some(nearest) = crate::suggest::closest(
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
/// more parameters than it passes leaves the extras unbound — a signature
/// mismatch, so it reports as 5002 like every other arity violation.
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

    use super::{analyze_builtin_function, builtin_catalog, synthetic_call};
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

    /// Sweeps every catalog entry through the dispatcher at several arities
    /// without panicking. This executes every leaf file's analyzer and
    /// signature.
    #[test]
    fn every_catalog_entry_analyzes_synthetic_calls() {
        let catalog = builtin_catalog();
        assert!(
            catalog.len() > 400,
            "catalog looks broken: only {} entries",
            catalog.len()
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
        for entry in catalog {
            let call = synthetic_call(entry.name);
            for args in &arg_shapes {
                let _ = analyze_builtin_function(&mut ctx, &call, args);
            }
            let _ = entry.signature();
        }
    }

    /// The catalog is the dispatch table, so the two cannot disagree — but
    /// the table itself has invariants: one row per name, every row in the
    /// family table that owns its prefix, and a real (non-synthetic) call to
    /// every current name reaching an analyzer rather than the 5001 fallback.
    #[test]
    fn the_catalog_is_well_formed_and_every_row_dispatches() {
        let catalog = builtin_catalog();
        let mut names = std::collections::BTreeSet::new();
        for entry in catalog {
            assert!(names.insert(entry.name), "duplicate entry `{}`", entry.name);
            assert!(
                entry.is_documented() || entry.name == "count::count",
                "`{}` has no doc; every documented spelling needs one",
                entry.name
            );
        }
        for (family, table) in super::FAMILIES.iter().enumerate() {
            let Some(head) = table.first().map(super::BuiltinEntry::family) else {
                panic!("family table {family} is empty");
            };
            for entry in *table {
                assert_eq!(
                    entry.family(),
                    head,
                    "`{}` sits in the `{head}` table",
                    entry.name
                );
            }
        }

        let schema = SchemaIndex::default();
        for entry in catalog {
            let mut diagnostics: Vec<Finding> = Vec::new();
            let source = entry.name;
            let mut ctx = AnalysisContext::new(
                &schema,
                surrealguard_syntax::source::SourceId::new("dispatch"),
                source,
                &mut diagnostics,
            );
            // A written (non-synthetic) call with no arguments: arity findings
            // are fine, an unknown-function finding is not.
            let call = ast::Call {
                path: ast::Spanned::new(
                    entry.name.to_string(),
                    surrealguard_syntax::span::ByteRange::new(0, source.len() as u32)
                        .expect("ordered"),
                ),
                args: Vec::new(),
            };
            let _ = analyze_builtin_function(&mut ctx, &call, &[]);
            assert!(
                diagnostics
                    .iter()
                    .all(|finding| finding.code().to_string() != "E5001"),
                "`{}` is in the catalog but dispatches to unknown-function",
                entry.name
            );
        }
    }

    /// Every current name in the version registry is a function the analyzer
    /// resolves, and every removed spelling the analyzer still dispatches is
    /// marked not-current, so completion never offers it.
    #[test]
    fn version_facts_and_the_catalog_agree() {
        for version in crate::analyzer::version::FUNCTIONS {
            // `type::is::x` never reaches dispatch: lowering folds it first.
            if version.name.contains("::is::") {
                continue;
            }
            if version.removed.is_none() {
                assert!(
                    super::builtin(version.name).is_some(),
                    "`{}` has version facts but no catalog entry",
                    version.name
                );
            }
        }
        for entry in builtin_catalog() {
            if let Some(version) = entry.version() {
                assert_eq!(
                    entry.is_current(),
                    version.removed.is_none(),
                    "`{}` current-ness must follow the version registry",
                    entry.name
                );
            }
        }
        assert!(!super::builtin("duration::from::days")
            .expect("dispatched for the rename hint")
            .is_current());
        assert!(super::builtin("duration::from_days")
            .expect("current spelling")
            .is_current());
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
            ("RETURN duration::from::days(3);", Kind::Duration),
            ("RETURN time::from::unix(1);", Kind::Datetime),
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
            matches!(response_kind_of(query), Some(Kind::Int | Kind::Any)),
            "cross-udf call must resolve to int or fall back to any, got {:?}",
            response_kind_of(query),
        );
        assert_eq!(diagnostics_of(query), Vec::new());
    }
}
