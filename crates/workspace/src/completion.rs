//! Type-aware completion.
//!
//! A pure, editor-independent engine: [`complete_at`] takes the analysis
//! facts the pipeline already produced plus a parse tree and a byte offset,
//! and returns a ranked candidate list. It computes no types of its own and
//! runs no analysis — every kind it shows was inferred by the analyzer and is
//! read out of [`AnalysisOutput`] / [`SchemaIndex`]. That is what makes it
//! safe to call on every keystroke.
//!
//! # Shape of the answer
//!
//! 1. [`context::classify`] decides *what kind of thing* goes at the cursor
//!    (see that module for why the classification is a token scan rather than
//!    a CST walk).
//! 2. The context selects which candidate families are in play and what kind
//!    the position expects.
//! 3. Every candidate is scored on four axes — how well its name matches what
//!    has been typed, whether its type fits the position, how relevant its
//!    family is to the context, and whether it is schema-local or a built-in —
//!    and the list is returned in descending score order with a `sort_text`
//!    that pins that order in the client.
//!
//! Type compatibility is a *primary* axis, not a tiebreak: in
//! `WHERE status = ▏` a `$status_filter` whose kind fits `status` outranks a
//! closer-spelled `$statistics` that does not.

pub(crate) mod builtins;
mod context;
mod lex;

use std::collections::BTreeMap;

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_syntax::parse::ParsedSource;

use crate::analysis::AnalysisOutput;
use crate::schema::SchemaIndex;

pub use context::{CompletionContext, ContextKind};

/// The most items ever returned. A completion payload is re-sent on every
/// keystroke, so an unfiltered 430-entry built-in dump would cost more in
/// transport than it buys in usefulness; ranking puts anything worth seeing
/// far above this cut.
const MAX_CANDIDATES: usize = 300;

/// What a candidate is, so the editor can pick an icon and the ranker can
/// weigh families against each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CandidateKind {
    /// A field of the row table, or a member of the receiver's kind.
    Field,
    /// A table name.
    Table,
    /// A `$param`: a `LET`/`FOR` binding, a host parameter, a `DEFINE PARAM`,
    /// or a context param bound by the enclosing DEFINE construct.
    Param,
    /// A callable: a `fn::` workspace function or a built-in.
    Function,
    /// A built-in reached as a method on a typed receiver (`.len()`).
    Method,
    /// A built-in family (`string::`), offered instead of its members until
    /// the family is chosen.
    Namespace,
    /// A key in a `CONTENT`/`MERGE` object literal.
    ObjectKey,
    /// A `DEFINE INDEX` name, offered after `WITH INDEX`.
    Index,
}

/// One ranked completion item.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionCandidate {
    /// What the editor shows in the list.
    pub label: String,
    /// What is written into the buffer when the item is accepted.
    pub insert_text: String,
    /// The candidate's class.
    pub kind: CandidateKind,
    /// The rendered type or signature, shown beside the label.
    pub detail: Option<String>,
    /// A longer explanation (the owning table, the binding's origin).
    pub documentation: Option<String>,
    /// The type this candidate has, when known — the axis type compatibility
    /// is scored on.
    pub candidate_kind: Option<Kind>,
    /// The combined score in `0.0..=1.0`.
    pub score: f32,
    /// The order key handed to the client, so it preserves this ranking
    /// instead of re-sorting alphabetically.
    pub sort_text: String,
    /// The byte range in the source the item replaces.
    pub replace: (u32, u32),
}

/// The classification at `offset`, without building candidates. Exposed so
/// context detection can be asserted directly.
pub fn completion_context_at(
    output: &AnalysisOutput,
    schema: &SchemaIndex,
    parsed: &ParsedSource,
    offset: u32,
) -> CompletionContext {
    let params = in_scope_params(output, schema, parsed, offset);
    context::classify(parsed, schema, &params.kinds(), offset)
}

/// Ranked completions at byte `offset` in `parsed`.
///
/// Reads only `output` and `schema`; it never re-analyzes, and it never
/// invents a name that is not in the schema or the analysis. Returns an empty
/// list where nothing can honestly be offered — inside a string literal, or
/// after a `.` whose receiver has no resolvable type (offering tables or
/// params there would be confidently wrong).
pub fn complete_at(
    output: &AnalysisOutput,
    schema: &SchemaIndex,
    parsed: &ParsedSource,
    offset: u32,
) -> Vec<CompletionCandidate> {
    let params = in_scope_params(output, schema, parsed, offset);
    let context = context::classify(parsed, schema, &params.kinds(), offset);
    if !context.enabled {
        return Vec::new();
    }

    let mut candidates: Vec<Draft> = Vec::new();
    let families = Families::for_context(context.kind);

    if families.members > 0.0 {
        if let Some(receiver) = &context.receiver {
            for (name, kind, owner) in members_of_kind(receiver, schema) {
                if context.exclude.contains(&name) {
                    continue;
                }
                candidates.push(member_candidate(name, kind, owner, families.members));
            }
        }
    }
    if families.methods > 0.0 {
        if let Some(receiver) = &context.receiver {
            candidates.extend(method_candidates(receiver, families.methods));
        }
    }
    if families.fields > 0.0 {
        for table in &context.tables {
            candidates.extend(field_candidates(schema, table, &context, families.fields));
        }
    }
    if families.tables > 0.0 {
        candidates.extend(table_candidates(schema, &context, families.tables));
    }
    if families.params > 0.0 {
        candidates.extend(params.candidates(families.params));
    }
    if families.functions > 0.0 {
        candidates.extend(function_candidates(schema, &context, families.functions));
    }
    if families.namespaces > 0.0 {
        candidates.extend(namespace_candidates(families.namespaces));
    }
    if families.aliases > 0.0 {
        candidates.extend(alias_candidates(&context, families.aliases));
    }
    if families.indexes > 0.0 {
        candidates.extend(index_candidates(schema, &context, families.indexes));
    }

    rank(candidates, &context)
}

// ---------------------------------------------------------------------------
// Family weights
// ---------------------------------------------------------------------------

/// How relevant each candidate family is to a context, in `0.0..=1.0`. Zero
/// excludes the family outright; the rest feeds the `family` axis of the
/// score, so a less-relevant family can still win on a strong name match.
#[derive(Clone, Copy, Debug, Default)]
struct Families {
    fields: f32,
    members: f32,
    methods: f32,
    tables: f32,
    params: f32,
    functions: f32,
    namespaces: f32,
    aliases: f32,
    indexes: f32,
}

impl Families {
    fn for_context(kind: ContextKind) -> Self {
        match kind {
            // A projection or predicate is overwhelmingly about fields, but a
            // param or a function call is legal there too.
            ContextKind::FieldName => Families {
                fields: 1.0,
                params: 0.5,
                functions: 0.3,
                namespaces: 0.3,
                ..Families::default()
            },
            // Only a table (or a param holding one) can appear here.
            ContextKind::TableName => Families {
                tables: 1.0,
                params: 0.4,
                ..Families::default()
            },
            // A graph slot takes a bare table name and nothing else: a param
            // cannot name an edge, and the set of legal tables is already
            // pinned by `allowed_tables`.
            ContextKind::EdgeTable | ContextKind::GraphNode => Families {
                tables: 1.0,
                ..Families::default()
            },
            // A group key labels a group, so only a projected alias or a row
            // field can stand here.
            ContextKind::GroupKey => Families {
                fields: 1.0,
                aliases: 1.0,
                ..Families::default()
            },
            ContextKind::IndexName => Families {
                indexes: 1.0,
                ..Families::default()
            },
            ContextKind::ObjectKey => Families {
                fields: 1.0,
                ..Families::default()
            },
            ContextKind::Member => Families {
                members: 1.0,
                methods: 0.45,
                ..Families::default()
            },
            // A destructure selects members only — a method call is not
            // valid inside `.{ … }`.
            ContextKind::Destructure => Families {
                members: 1.0,
                ..Families::default()
            },
            ContextKind::ParamName => Families {
                params: 1.0,
                ..Families::default()
            },
            ContextKind::FunctionPath => Families {
                functions: 1.0,
                namespaces: 0.8,
                ..Families::default()
            },
            // Anything with a value can go here; params lead because they are
            // the position's most common filler.
            ContextKind::Value => Families {
                params: 1.0,
                fields: 0.6,
                functions: 0.55,
                namespaces: 0.55,
                tables: 0.2,
                ..Families::default()
            },
            // The honest fallback: everything nameable, evenly weighted, so a
            // context we could not classify still answers usefully.
            ContextKind::Unknown => Families {
                params: 0.7,
                fields: 0.55,
                tables: 0.5,
                functions: 0.5,
                namespaces: 0.5,
                ..Families::default()
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Candidate sources
// ---------------------------------------------------------------------------

/// A candidate before ranking, carrying the family weight its source assigned.
/// The weight is an input to the score, not the score itself, so it is kept
/// beside the candidate rather than stashed in its `score` field.
struct Draft {
    candidate: CompletionCandidate,
    family: f32,
}

impl Draft {
    fn new(candidate: CompletionCandidate, family: f32) -> Self {
        Self { candidate, family }
    }
}

/// The declared fields of `table`, plus the implicit `id` every record has
/// and the `in`/`out` a `TYPE RELATION` edge has.
fn field_candidates(
    schema: &SchemaIndex,
    table: &str,
    context: &CompletionContext,
    weight: f32,
) -> Vec<Draft> {
    let Some(definition) = schema.table(table) else {
        return Vec::new();
    };
    let object_key = context.kind == ContextKind::ObjectKey;
    let mut out = Vec::new();
    for field in definition.fields.values() {
        // Only the immediate fields: `address.city` is reached by completing
        // `address` and then `.`.
        if field.path.len() != 1 {
            continue;
        }
        let name = &field.path[0];
        if context.exclude.contains(name) {
            continue;
        }
        // A `CONTENT`/`MERGE` key that cannot be written is not a candidate.
        if object_key && (field.computed || field.readonly) {
            continue;
        }
        out.push(Draft::new(
            CompletionCandidate {
                label: name.clone(),
                insert_text: name.clone(),
                kind: if object_key {
                    CandidateKind::ObjectKey
                } else {
                    CandidateKind::Field
                },
                detail: field.kind.as_ref().map(crate::render_kind),
                documentation: Some(format!("field of `{table}`")),
                candidate_kind: field.kind.clone(),
                score: 0.0,
                sort_text: String::new(),
                replace: context.replace,
            },
            weight,
        ));
    }
    // The implicit record fields are real and queryable, but live outside
    // `fields` so the schemaless gate stays untouched.
    for implicit in ["id", "in", "out"] {
        let Some(kind) = definition.implicit_field_kind(implicit) else {
            continue;
        };
        if context.exclude.contains(implicit) || (object_key && implicit == "id") {
            continue;
        }
        out.push(Draft::new(
            CompletionCandidate {
                label: implicit.to_string(),
                insert_text: implicit.to_string(),
                kind: if object_key {
                    CandidateKind::ObjectKey
                } else {
                    CandidateKind::Field
                },
                detail: Some(crate::render_kind(&kind)),
                documentation: Some(format!("implicit field of `{table}`")),
                candidate_kind: Some(kind),
                score: 0.0,
                sort_text: String::new(),
                replace: context.replace,
            },
            weight,
        ));
    }
    out
}

fn member_candidate(name: String, kind: Option<Kind>, owner: Option<String>, weight: f32) -> Draft {
    Draft::new(
        CompletionCandidate {
            label: name.clone(),
            insert_text: name,
            kind: CandidateKind::Field,
            detail: kind.as_ref().map(crate::render_kind),
            documentation: owner.map(|table| format!("field of `{table}`")),
            candidate_kind: kind,
            score: 0.0,
            sort_text: String::new(),
            replace: (0, 0),
        },
        weight,
    )
}

/// Every table the position admits, with its relation spec as detail when it
/// is an edge.
///
/// A graph slot admits only the tables its receiver can actually reach, and
/// `allowed_tables` has already resolved that set — so this is a *filter*.
/// Ranking a wrong edge below a right one still offers it, and an offered
/// traversal that returns nothing at runtime is worse than no offer at all.
fn table_candidates(schema: &SchemaIndex, context: &CompletionContext, weight: f32) -> Vec<Draft> {
    schema
        .tables
        .values()
        .filter(|table| match &context.allowed_tables {
            Some(allowed) => allowed.iter().any(|name| name == &table.name),
            None => true,
        })
        .map(|table| {
            let detail = match &table.relation {
                Some(relation) => Some(format!(
                    "relation {} -> {}",
                    join_or(&relation.in_tables),
                    join_or(&relation.out_tables)
                )),
                None => Some("table".to_string()),
            };
            Draft::new(
                CompletionCandidate {
                    label: table.name.clone(),
                    insert_text: table.name.clone(),
                    kind: CandidateKind::Table,
                    detail,
                    documentation: Some(format!(
                        "{} fields",
                        table.fields.values().filter(|f| f.path.len() == 1).count()
                    )),
                    candidate_kind: Some(Kind::Record(vec![surrealdb_types::Table::from(
                        table.name.as_str(),
                    )])),
                    score: 0.0,
                    sort_text: String::new(),
                    replace: (0, 0),
                },
                weight,
            )
        })
        .collect()
}

/// The aliases the enclosing projection binds. A `GROUP BY` can name one of
/// these, and `W4013` says a key that is *not* projected is a mistake — so
/// they lead the group-key offer.
fn alias_candidates(context: &CompletionContext, weight: f32) -> Vec<Draft> {
    context
        .aliases
        .iter()
        .filter(|alias| !context.exclude.contains(*alias))
        .map(|alias| {
            Draft::new(
                CompletionCandidate {
                    label: alias.clone(),
                    insert_text: alias.clone(),
                    kind: CandidateKind::Field,
                    detail: None,
                    documentation: Some("projection alias".to_string()),
                    candidate_kind: None,
                    score: 0.0,
                    sort_text: String::new(),
                    replace: (0, 0),
                },
                weight,
            )
        })
        .collect()
}

/// The indexes defined on the queried tables. Only these can follow
/// `WITH INDEX`: an index on another table is not a candidate at all.
fn index_candidates(schema: &SchemaIndex, context: &CompletionContext, weight: f32) -> Vec<Draft> {
    let mut out = Vec::new();
    for table in &context.tables {
        let Some(definition) = schema.table(table) else {
            continue;
        };
        for index in definition.indexes.values() {
            if context.exclude.contains(&index.name) {
                continue;
            }
            out.push(Draft::new(
                CompletionCandidate {
                    label: index.name.clone(),
                    insert_text: index.name.clone(),
                    kind: CandidateKind::Index,
                    detail: Some(format!(
                        "{} index on {}",
                        index_kind_text(index.kind),
                        index.field_paths().join(", ")
                    )),
                    documentation: Some(format!("index of `{table}`")),
                    candidate_kind: None,
                    score: 0.0,
                    sort_text: String::new(),
                    replace: (0, 0),
                },
                weight,
            ));
        }
    }
    out
}

fn index_kind_text(kind: crate::schema::IndexKind) -> &'static str {
    match kind {
        crate::schema::IndexKind::Normal => "plain",
        crate::schema::IndexKind::Unique => "unique",
        crate::schema::IndexKind::Search => "full-text",
        crate::schema::IndexKind::Vector => "vector",
    }
}

/// Built-ins that read a match reference produced by an index-backed
/// operator, so they mean nothing without a full-text index on the row table
/// — the same requirement `E1027` enforces on `@@`.
const FULLTEXT_ONLY: &[&str] = &["search::score", "search::highlight", "search::offsets"];

/// Whether any of `tables` carries a full-text index. An unknown row table
/// answers `true`: an unprovable requirement must not hide a valid call.
fn has_fulltext_index(schema: &SchemaIndex, tables: &[String]) -> bool {
    tables.is_empty()
        || tables.iter().any(|name| {
            schema.table(name).is_some_and(|table| {
                table
                    .indexes
                    .values()
                    .any(|index| index.kind == crate::schema::IndexKind::Search)
            })
        })
}

fn join_or(tables: &[String]) -> String {
    if tables.is_empty() {
        "any".to_string()
    } else {
        tables.join(" | ")
    }
}

/// `fn::` workspace functions always; built-ins only once something has been
/// typed (an unfiltered dump of 400+ names would bury the local schema).
fn function_candidates(
    schema: &SchemaIndex,
    context: &CompletionContext,
    weight: f32,
) -> Vec<Draft> {
    let mut out: Vec<Draft> = schema
        .functions
        .values()
        .map(|function| {
            let returns = function
                .return_kind
                .clone()
                .or_else(|| function.inferred_return.clone());
            Draft::new(
                CompletionCandidate {
                    label: function.name.clone(),
                    insert_text: function.name.clone(),
                    kind: CandidateKind::Function,
                    detail: Some(function_signature(function, returns.as_ref())),
                    documentation: Some("workspace function".to_string()),
                    candidate_kind: returns,
                    score: 0.0,
                    sort_text: String::new(),
                    replace: (0, 0),
                },
                weight,
            )
        })
        .collect();

    if !context.prefix.is_empty() {
        let fulltext = has_fulltext_index(schema, &context.tables);
        out.extend(
            builtins::offered()
                .filter(|builtin| fulltext || !FULLTEXT_ONLY.contains(&builtin.name))
                .map(|builtin| {
                    Draft::new(
                        CompletionCandidate {
                            label: builtin.name.to_string(),
                            insert_text: builtin.name.to_string(),
                            kind: CandidateKind::Function,
                            detail: Some(builtins::signature_text(builtin)),
                            documentation: Some(builtin.doc.to_string()),
                            candidate_kind: builtins::return_kind(builtin),
                            score: 0.0,
                            sort_text: String::new(),
                            replace: (0, 0),
                        },
                        weight,
                    )
                }),
        );
    }
    out
}

/// `fn::name($a: kind, …) -> kind`, matching the hover popover's rendering.
fn function_signature(function: &crate::schema::FunctionDef, returns: Option<&Kind>) -> String {
    let params: Vec<String> = function
        .args
        .iter()
        .map(|arg| {
            let kind = arg
                .kind
                .as_ref()
                .map_or_else(|| "any".to_string(), crate::render_kind);
            format!("${}: {kind}", arg.name)
        })
        .collect();
    let mut signature = format!("{}({})", function.name, params.join(", "));
    if let Some(returns) = returns {
        signature.push_str(&format!(" -> {}", crate::render_kind(returns)));
    }
    signature
}

/// The built-in families, offered as `string::`-style entries so the first
/// keystroke narrows to a family rather than scrolling 400 functions.
fn namespace_candidates(weight: f32) -> Vec<Draft> {
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for builtin in builtins::offered() {
        *counts.entry(builtin.family()).or_default() += 1;
    }
    let mut out = Vec::new();
    for (family, count) in counts {
        // A single-function family (`count`, `rand`) *is* the function, and is
        // already offered as one.
        if count == 1 {
            continue;
        }
        out.push(Draft::new(
            CompletionCandidate {
                label: format!("{family}::"),
                insert_text: format!("{family}::"),
                kind: CandidateKind::Namespace,
                detail: Some(format!("{count} functions")),
                documentation: Some("built-in function family".to_string()),
                candidate_kind: None,
                score: 0.0,
                sort_text: String::new(),
                replace: (0, 0),
            },
            weight,
        ));
    }
    out
}

/// Built-ins reachable as method-call sugar on `receiver`, dispatched by the
/// receiver's kind family exactly as the analyzer dispatches them.
fn method_candidates(receiver: &Kind, weight: f32) -> Vec<Draft> {
    let Some(family) = method_family(receiver) else {
        return Vec::new();
    };
    builtins::offered()
        .filter(|builtin| builtin.family() == family)
        .filter_map(|builtin| {
            let method = builtin.name.strip_prefix(family)?.strip_prefix("::")?;
            // A nested path (`array::sort::asc`) is not a method: SurrealQL
            // has no `.sort::asc()` sugar.
            if method.contains("::") {
                return None;
            }
            Some(Draft::new(
                CompletionCandidate {
                    label: method.to_string(),
                    insert_text: method.to_string(),
                    kind: CandidateKind::Method,
                    detail: Some(builtins::signature_text(builtin)),
                    documentation: Some(builtin.doc.to_string()),
                    candidate_kind: builtins::method_return_kind(builtin, receiver),
                    score: 0.0,
                    sort_text: String::new(),
                    replace: (0, 0),
                },
                weight,
            ))
        })
        .collect()
}

/// The built-in family a method call on this kind dispatches to. Mirrors
/// `analyzer::expression::infer::method_return_kind`, so completion offers
/// exactly the methods the analyzer would resolve.
fn method_family(receiver: &Kind) -> Option<&'static str> {
    let base = crate::kinds::literal_base_kind(receiver).unwrap_or_else(|| receiver.clone());
    // Only optionality is transparent to dispatch: `option<string>` has the
    // string methods, but `array<string>` has the *array* ones, so the
    // collection wrappers must survive.
    let (wrappers, payload) = crate::kinds::peel_wrappers(&base);
    let base = if wrappers
        .iter()
        .all(|wrapper| matches!(wrapper, crate::kinds::KindWrapper::Optional))
    {
        payload
    } else {
        base
    };
    Some(match base {
        Kind::Array(_, _) => "array",
        Kind::Set(_, _) => "set",
        Kind::String => "string",
        Kind::Object => "object",
        Kind::Duration => "duration",
        Kind::Datetime => "time",
        Kind::Bytes => "bytes",
        Kind::Record(_) => "record",
        Kind::Int | Kind::Float | Kind::Decimal | Kind::Number => "math",
        _ => return None,
    })
}

/// One member of a receiver's kind: its name, its type when known, and the
/// table that declares it (absent for a literal object's entries).
type Member = (String, Option<Kind>, Option<String>);

/// The members of a kind: the fields of every table a record link points at,
/// or the entries of a closed literal object, seen through `option`/`array`
/// wrappers and across union arms.
fn members_of_kind(kind: &Kind, schema: &SchemaIndex) -> Vec<Member> {
    let (_, payload) = crate::kinds::peel_wrappers(kind);
    match payload {
        Kind::Record(targets) => targets
            .iter()
            .filter_map(|target| schema.table(&target.to_string()))
            .flat_map(|table| {
                let owner = table.name.clone();
                let declared = table
                    .fields
                    .values()
                    .filter(|field| field.path.len() == 1)
                    .map(move |field| {
                        (
                            field.path[0].clone(),
                            field.kind.clone(),
                            Some(field.table.clone()),
                        )
                    });
                let implicit = ["id", "in", "out"].into_iter().filter_map({
                    let table = table.clone();
                    let owner = owner.clone();
                    move |name| {
                        table
                            .implicit_field_kind(name)
                            .map(|kind| (name.to_string(), Some(kind), Some(owner.clone())))
                    }
                });
                declared.collect::<Vec<_>>().into_iter().chain(implicit)
            })
            .collect(),
        Kind::Literal(KindLiteral::Object(entries)) => entries
            .into_iter()
            .map(|(name, kind)| (name, Some(kind), None))
            .collect(),
        // A multi-arm union was not peeled; take the members every arm shares
        // the name of, so nothing offered is missing on one of the arms.
        Kind::Either(variants) => {
            let per_arm: Vec<Vec<Member>> = variants
                .iter()
                .filter(|variant| !matches!(variant, Kind::None | Kind::Null))
                .map(|variant| members_of_kind(variant, schema))
                .collect();
            let Some((first, rest)) = per_arm.split_first() else {
                return Vec::new();
            };
            first
                .iter()
                .filter(|(name, _, _)| {
                    rest.iter()
                        .all(|arm| arm.iter().any(|(other, _, _)| other == name))
                })
                .cloned()
                .collect()
        }
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Parameters in scope
// ---------------------------------------------------------------------------

/// Where a `$param` came from, which drives both its detail line and whether
/// it counts as schema-local.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParamOrigin {
    /// A `LET $x = …` or `FOR $x IN …` binding in this source.
    Binding,
    /// A parameter the host must supply.
    Host,
    /// A `DEFINE PARAM`.
    Defined,
    /// A context param the enclosing DEFINE construct binds (`$after`,
    /// `$value`, `$auth`, …).
    Context,
}

impl ParamOrigin {
    fn description(self) -> &'static str {
        match self {
            Self::Binding => "local binding",
            Self::Host => "host parameter",
            Self::Defined => "DEFINE PARAM",
            Self::Context => "context parameter",
        }
    }
}

/// The `$param`s visible at the cursor.
struct ScopedParams {
    entries: Vec<(String, Option<Kind>, ParamOrigin)>,
}

impl ScopedParams {
    /// The subset with a known kind, as the map resolution needs.
    fn kinds(&self) -> BTreeMap<String, Kind> {
        self.entries
            .iter()
            .filter_map(|(name, kind, _)| Some((name.clone(), kind.clone()?)))
            .collect()
    }

    fn candidates(&self, weight: f32) -> Vec<Draft> {
        self.entries
            .iter()
            .map(|(name, kind, origin)| {
                Draft::new(
                    CompletionCandidate {
                        label: format!("${name}"),
                        insert_text: format!("${name}"),
                        kind: CandidateKind::Param,
                        detail: kind.as_ref().map(crate::render_kind),
                        documentation: Some(origin.description().to_string()),
                        candidate_kind: kind.clone(),
                        score: 0.0,
                        sort_text: String::new(),
                        replace: (0, 0),
                    },
                    weight,
                )
            })
            .collect()
    }
}

/// Collects every `$param` in scope at `offset`, newest binding winning on a
/// name collision.
///
/// A `LET` is in scope only if it was written before the cursor *and* the
/// block it lives in still encloses the cursor — a binding inside a `fn::`
/// body that already closed is not visible to a later statement.
fn in_scope_params(
    output: &AnalysisOutput,
    schema: &SchemaIndex,
    parsed: &ParsedSource,
    offset: u32,
) -> ScopedParams {
    let mut seen: BTreeMap<String, (Option<Kind>, ParamOrigin)> = BTreeMap::new();

    // The session/access params are seeded into every top-level query env and
    // every `fn::` body, so they are in scope everywhere.
    for (name, kind) in crate::context_params::session_context_params() {
        seen.insert(name, (Some(kind), ParamOrigin::Context));
    }

    // Context params come next so a real binding of the same name overrides
    // them below. Lowering is the accurate source; when the construct's body
    // is too incomplete to lower — which is the normal state while typing in
    // it — the construct is recovered from tokens instead.
    let context_params = crate::context_params::context_param_map_in(schema, parsed, offset)
        .or_else(|| {
            let shape = context::enclosing_define(parsed.text(), offset)?;
            crate::context_params::context_param_map_for(
                schema,
                &shape.keyword,
                &shape.name,
                shape.table.as_deref(),
            )
        });
    if let Some(map) = context_params {
        for (name, kind) in map {
            seen.insert(name, (Some(kind), ParamOrigin::Context));
        }
    }
    for param in schema.params.values() {
        seen.entry(param.name.clone())
            .or_insert((None, ParamOrigin::Defined));
    }
    for param in &output.inferred_params {
        // A context param is bound by the engine, so nothing the host supplies
        // can shadow it. Without this guard an in-progress `$this->` — which
        // the analyzer cannot yet resolve and records as an untyped host param
        // — would erase the `record<table>` the enclosing DEFINE gives it, and
        // with it the receiver every graph step needs.
        if matches!(seen.get(&param.name), Some((_, ParamOrigin::Context))) {
            continue;
        }
        seen.insert(param.name.clone(), (param.kind.clone(), ParamOrigin::Host));
    }

    // A guard that narrowed a symbol narrowed it for the completion list too.
    // Hover has read `output.narrowings` since it was recorded; completion read
    // the declared kind, so `$x.` past `IF $x = NONE THEN THROW` offered the
    // fields of an `option<record<t>>` — which is to say none — while hovering
    // the same token two columns left said `record<t>`. One analysis, two
    // answers, and the one the user sees first was the wrong one.
    let closed = closed_block_ranges(parsed.text(), offset);
    for binding in &output.let_bindings {
        if binding.name_span.source() != parsed.source_id() {
            continue;
        }
        let start = binding.name_span.range().start();
        if start >= offset
            || closed
                .iter()
                .any(|(from, to)| start >= *from && start < *to)
        {
            continue;
        }
        seen.insert(
            binding.name.clone(),
            (binding.kind.clone(), ParamOrigin::Binding),
        );
    }

    // Applied last, over every origin: a narrowing is a statement about this
    // program point, and it outranks whatever bound the name.
    for (name, (kind, _)) in &mut seen {
        if let Some(narrowing) =
            crate::query::narrowing_at(&output.narrowings, parsed.source_id(), name, offset)
        {
            *kind = Some(narrowing.kind.clone());
        }
    }

    ScopedParams {
        entries: seen
            .into_iter()
            .map(|(name, (kind, origin))| (name, kind, origin))
            .collect(),
    }
}

/// Byte ranges of `{ … }` blocks that opened *and closed* before `offset`.
/// Anything bound inside one has gone out of scope.
fn closed_block_ranges(source: &str, offset: u32) -> Vec<(u32, u32)> {
    let tokens = lex::tokenize(source);
    let mut stack: Vec<u32> = Vec::new();
    let mut closed = Vec::new();
    for token in &tokens {
        if token.start >= offset {
            break;
        }
        if token.kind != lex::TokenKind::Punct {
            continue;
        }
        match token.text(source) {
            "{" => stack.push(token.start),
            "}" => {
                if let Some(open) = stack.pop() {
                    closed.push((open, token.end));
                }
            }
            _ => {}
        }
    }
    closed
}

// ---------------------------------------------------------------------------
// Ranking
// ---------------------------------------------------------------------------

/// How much each axis contributes. Name match leads, but type fit is close
/// behind and decides between similarly-spelled candidates — the difference
/// between an incompatible and a compatible type (0.05 vs 0.9) is 0.30 of the
/// final score, more than a prefix match is worth over a scattered one.
const WEIGHT_MATCH: f32 = 0.45;
const WEIGHT_TYPE: f32 = 0.35;
const WEIGHT_FAMILY: f32 = 0.15;
const WEIGHT_LOCAL: f32 = 0.05;

/// Scores, filters, sorts, and assigns `sort_text`.
fn rank(drafts: Vec<Draft>, context: &CompletionContext) -> Vec<CompletionCandidate> {
    let mut scored: Vec<CompletionCandidate> = drafts
        .into_iter()
        .filter_map(
            |Draft {
                 mut candidate,
                 family,
             }| {
                let name_score = match_score(&context.prefix, &candidate.label)?;
                let type_score =
                    type_score(candidate.candidate_kind.as_ref(), context.expected.as_ref());
                let local = f32::from(u8::from(is_schema_local(&candidate)));
                candidate.score = WEIGHT_MATCH * name_score
                    + WEIGHT_TYPE * type_score
                    + WEIGHT_FAMILY * family
                    + WEIGHT_LOCAL * local;
                candidate.replace = context.replace;
                Some(candidate)
            },
        )
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.label.cmp(&b.label))
    });
    scored.truncate(MAX_CANDIDATES);
    for (index, candidate) in scored.iter_mut().enumerate() {
        candidate.sort_text = format!("{index:04}");
    }
    scored
}

/// Whether a candidate comes from this workspace rather than the built-in
/// surface. In a project's own query, the project's own names are almost
/// always what was meant, so they carry a small standing advantage.
fn is_schema_local(candidate: &CompletionCandidate) -> bool {
    match candidate.kind {
        CandidateKind::Namespace | CandidateKind::Method => false,
        CandidateKind::Function => candidate.label.starts_with("fn::"),
        CandidateKind::Field
        | CandidateKind::Table
        | CandidateKind::Param
        | CandidateKind::ObjectKey
        | CandidateKind::Index => true,
    }
}

/// How well `label` matches what has been typed, in `0.0..=1.0`. `None` means
/// the label does not match at all and is filtered out.
///
/// The tiers, strongest first: exact, prefix, a substring starting at a word
/// boundary (`len` in `string::len`), any substring, then a scattered
/// subsequence scored by how many of its characters landed on word
/// boundaries — so `sl` prefers `string::len` over `sales_total`.
fn match_score(prefix: &str, label: &str) -> Option<f32> {
    if prefix.is_empty() {
        return Some(0.5);
    }
    let needle = prefix.to_ascii_lowercase();
    let hay = label.to_ascii_lowercase();
    // A `$` in the typed text has already been stripped; strip it from the
    // label too so `$or` matches `$organization`.
    let hay = hay.strip_prefix('$').unwrap_or(&hay).to_string();
    let label_no_sigil = label.strip_prefix('$').unwrap_or(label);

    if hay == needle {
        return Some(1.0);
    }
    if hay.starts_with(&needle) {
        return Some(if label_no_sigil.starts_with(prefix) {
            0.97
        } else {
            0.93
        });
    }
    if let Some(position) = hay.find(&needle) {
        let boundary = is_boundary(&hay, position);
        // Later matches are weaker: a hit at the start of the last segment
        // still reads as "the thing I meant".
        let decay = 0.05 * (position as f32 / hay.len().max(1) as f32);
        return Some(if boundary { 0.82 - decay } else { 0.68 - decay });
    }

    // Scattered subsequence.
    let mut hay_chars = hay.char_indices();
    let mut boundary_hits = 0usize;
    let mut matched = 0usize;
    for wanted in needle.chars() {
        let found = hay_chars.find(|(_, character)| *character == wanted)?;
        matched += 1;
        if is_boundary(&hay, found.0) {
            boundary_hits += 1;
        }
    }
    let density = boundary_hits as f32 / matched.max(1) as f32;
    Some(0.30 + 0.28 * density)
}

/// Whether `position` starts a word inside `text`: the beginning, or just
/// after a separator.
fn is_boundary(text: &str, position: usize) -> bool {
    if position == 0 {
        return true;
    }
    text[..position]
        .chars()
        .next_back()
        .is_some_and(|previous| matches!(previous, '_' | ':' | '.' | '-' | '/' | ' '))
}

/// How well a candidate's type fits the position, in `0.0..=1.0`.
///
/// A position with no expectation, or a candidate with no known type, scores
/// neutrally — an unknown type must never be punished like a wrong one. A
/// genuine mismatch scores near zero, which is what lets type compatibility
/// reorder an otherwise better-spelled candidate.
fn type_score(candidate: Option<&Kind>, expected: Option<&Kind>) -> f32 {
    let Some(expected) = expected else {
        return 0.5;
    };
    if matches!(expected, Kind::Any) {
        return 0.55;
    }
    let Some(candidate) = candidate else {
        return 0.4;
    };
    if candidate == expected {
        return 1.0;
    }
    if crate::kinds::kind_is_assignable_to(candidate, expected) {
        return 0.9;
    }
    // The other direction is a *possible* fit — an `any`-ish candidate, or a
    // wider kind that may narrow at runtime — so it beats a mismatch without
    // beating a real fit.
    if crate::kinds::kind_is_assignable_to(expected, candidate) {
        return 0.65;
    }
    0.05
}

#[cfg(test)]
mod tests;
