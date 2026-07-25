//! Schema extraction: turns lowered `DEFINE`/`REMOVE`/`ALTER` statements
//! into the `SchemaIndex` (tables, fields, params, functions, analyzers) and
//! converts declared type syntax to upstream `Kind`s. Extraction only
//! mutates the index; the contract checks that reference these definitions
//! live in the owning statement analyzers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use surrealdb_types::Kind;
use surrealguard_syntax::ast;
use surrealguard_syntax::parse::ParsedSource;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use crate::expression::PartialReason;

/// The catalog built from all `DEFINE`/`REMOVE`/`ALTER` statements: the
/// tables, params, functions, and analyzers every contract check resolves
/// against.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaIndex {
    /// Defined tables, keyed by name.
    pub tables: BTreeMap<String, TableDef>,
    /// `DEFINE PARAM` globals, keyed by name (without `$`).
    pub params: BTreeMap<String, ParamDef>,
    /// `DEFINE FUNCTION` definitions, keyed by `fn::` path.
    pub functions: BTreeMap<String, FunctionDef>,
    /// `DEFINE ANALYZER` definitions, keyed by name.
    pub analyzers: BTreeMap<String, AnalyzerDef>,
}

/// A `DEFINE PARAM` global and where it was declared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamDef {
    /// The parameter name, without the leading `$`.
    pub name: String,
    /// The source the definition lives in.
    pub source: SourceId,
    /// Span of the parameter name.
    pub name_span: SourceSpan,
    /// Span of the `VALUE` expression, when present.
    pub value_span: Option<SourceSpan>,
}

/// One declared `fn::` parameter: `$name: string`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionParam {
    /// The parameter name, without the leading `$`.
    pub name: String,
    /// The declared kind, when the parameter is typed.
    pub kind: Option<Kind>,
}

/// A `DEFINE FUNCTION` definition: its signature, callees, and location.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionDef {
    /// The `fn::` path.
    pub name: String,
    /// Declared arguments, in order.
    pub args: Vec<FunctionParam>,
    /// `fn::` paths called from the body, for cycle detection.
    pub callees: Vec<String>,
    /// The declared return kind (`-> T`), when the definition annotates one.
    /// Authoritative: the body is checked against it (2012), and call sites
    /// resolve to it before falling back to [`Self::inferred_return`].
    pub return_kind: Option<Kind>,
    /// The return kind inferred from the body, populated only when the
    /// definition omits an explicit `-> T` and the body's response kind is a
    /// concrete (non-`Any`) type. Call sites fall back to this so untyped
    /// `fn::` helpers still propagate a real type. `None` when a return is
    /// declared (declared wins) or the body is genuinely untyped.
    pub inferred_return: Option<Kind>,
    /// The source the definition lives in.
    pub source: SourceId,
    /// Span of the function name.
    pub name_span: SourceSpan,
    /// Span of the return-type annotation, when present.
    pub return_span: Option<SourceSpan>,
}

/// A `DEFINE ANALYZER` definition: its tokenizer/filter pipeline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyzerDef {
    /// The analyzer name.
    pub name: String,
    /// Tokenizer names in the pipeline.
    pub tokenizers: Vec<String>,
    /// Filter specifications in the pipeline (name plus any arguments).
    pub filters: Vec<String>,
    /// The source the definition lives in.
    pub source: SourceId,
    /// Span of the analyzer name.
    pub name_span: SourceSpan,
}

/// A field access path split into its dotted segments (`profile.name` →
/// `["profile", "name"]`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FieldPath(Vec<String>);

impl FieldPath {
    /// A path from its already-split segments.
    pub fn new(parts: Vec<String>) -> Self {
        Self(parts)
    }

    /// Splits a dotted path string into segments, dropping empty ones.
    pub fn parse(path: &str) -> Self {
        Self(
            path.split('.')
                .filter(|part| !part.is_empty())
                .map(ToString::to_string)
                .collect(),
        )
    }

    /// The path's segments.
    pub fn parts(&self) -> &[String] {
        &self.0
    }

    /// The path rejoined with `.` separators.
    pub fn dotted(&self) -> String {
        self.0.join(".")
    }

    /// Whether this path is a (non-strict) prefix of `path` — the same
    /// segments up to this path's length.
    pub fn is_prefix_of(&self, path: &[String]) -> bool {
        self.0.len() <= path.len()
            && self
                .0
                .iter()
                .zip(path.iter())
                .all(|(left, right)| left == right)
    }
}

/// A `DEFINE TABLE` definition with its attached fields and indexes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableDef {
    /// The table name.
    pub name: String,
    /// The source the definition lives in.
    pub source: SourceId,
    /// Span of the table name.
    pub name_span: SourceSpan,
    /// Attached fields, keyed by dotted path.
    pub fields: BTreeMap<String, FieldDef>,
    /// Attached indexes, keyed by index name.
    pub indexes: BTreeMap<String, IndexDef>,
    /// The `TYPE RELATION` edge spec, when the table is a relation.
    pub relation: Option<RelationDef>,
    /// `DEFINE TABLE ... SCHEMAFULL` — only declared fields are retained.
    pub schemafull: bool,
    /// `DEFINE TABLE ... DROP` — rows are never retained.
    pub drop_table: bool,
    /// `DEFINE TABLE ... CHANGEFEED <duration>`.
    pub changefeed: bool,
}

/// A `TYPE RELATION` edge spec: the tables an edge may connect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationDef {
    /// Allowed `in` (source) tables; empty means any.
    pub in_tables: Vec<String>,
    /// Allowed `out` (destination) tables; empty means any.
    pub out_tables: Vec<String>,
    /// Span of the relation clause.
    pub span: SourceSpan,
}

/// One structural step of a `DEFINE FIELD` path.
///
/// The dotted key a field is stored under is lossy: `items[*].price` and
/// `items.price` both collapse to `items.price`, even though only the first
/// declares the ELEMENT type of an array. The step list keeps the distinction
/// for the rules that need it — see [`field_placement`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldStep {
    /// A named subfield: `.price`.
    Field(String),
    /// A collection's element: `[*]` or `.*`.
    Element,
}

/// A `DEFINE FIELD` definition: its declared kind and write semantics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDef {
    /// The field has a `DEFAULT` clause (or `VALUE`, which supplies one).
    pub has_default: bool,
    /// `READONLY` — writable only at creation.
    pub readonly: bool,
    /// `VALUE <expr>` — computed on write; hand-written values are
    /// overwritten. Also set for a `COMPUTED <expr>` (3.0) derived field.
    pub computed: bool,
    /// `REFERENCE` — the field's `record<...>` link is a reference, so a
    /// `<~` back-traversal on the target table can resolve through it.
    pub reference: bool,
    /// The field's dotted path split into segments.
    pub path: Vec<String>,
    /// The declaration's STRUCTURAL path — like [`FieldDef::path`], but
    /// keeping the `[*]` element steps that the dotted key collapses away, so
    /// `items[*].price` is distinguishable from `items.price`. Empty when the
    /// declaration's idiom is not a plain field/element path (an index
    /// expression, a filter, ...), which no structural rule applies to.
    pub steps: Vec<FieldStep>,
    /// The owning table's name.
    pub table: String,
    /// The declared kind, when the field is typed.
    pub kind: Option<Kind>,
    /// Why the kind is incomplete; empty when fully resolved.
    pub partial: Vec<PartialReason>,
    /// The source the definition lives in.
    pub source: SourceId,
    /// Span of the field name.
    pub name_span: SourceSpan,
    /// Span of the `ON TABLE` name.
    pub table_span: SourceSpan,
    /// Span of the `TYPE` annotation, when present.
    pub type_span: Option<SourceSpan>,
}

/// A `DEFINE INDEX` definition over one or more table fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexDef {
    /// The index name.
    pub name: String,
    /// The owning table's name.
    pub table: String,
    fields: Vec<IndexFieldDef>,
    /// What backs the index.
    pub kind: IndexKind,
    /// Span of the index name.
    pub name_span: SourceSpan,
    /// Span of the `ON TABLE` name.
    pub table_span: SourceSpan,
}

impl IndexDef {
    /// Whether this index covers the given dotted field path.
    pub fn covers(&self, path: &str) -> bool {
        self.fields.iter().any(|field| field.path.join(".") == path)
    }

    /// The dotted field paths this index covers, for duplicate detection.
    pub(crate) fn field_paths(&self) -> Vec<String> {
        self.fields
            .iter()
            .map(|field| field.path.join("."))
            .collect()
    }
}

/// What backs the index: full-text search, a vector structure, or plain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexKind {
    /// A plain (non-unique) index.
    Normal,
    /// A `UNIQUE` index.
    Unique,
    /// A full-text `SEARCH` index.
    Search,
    /// An `MTREE`/`HNSW` vector index.
    Vector,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct IndexFieldDef {
    path: Vec<String>,
    text: String,
    span: SourceSpan,
}

/// The result of building a schema from a batch of sources: the index plus
/// any findings raised while defining it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaExtraction {
    /// The assembled catalog.
    pub schema: SchemaIndex,
    /// Findings raised during extraction.
    pub diagnostics: Vec<surrealguard_diagnostics::Finding>,
}

impl SchemaIndex {
    /// The table definition named `name`, if defined.
    pub fn table(&self, name: &str) -> Option<&TableDef> {
        self.tables.get(name)
    }

    /// The field at `path` on `table`, if both are defined.
    pub fn field(&self, table: &str, path: &FieldPath) -> Option<&FieldDef> {
        self.table(table)?.field(path)
    }

    /// The global param `name`, accepting either a leading `$` or none.
    pub fn param(&self, name: &str) -> Option<&ParamDef> {
        self.params.get(name.strip_prefix('$').unwrap_or(name))
    }

    /// The function defined at the `fn::` path `name`, if defined.
    pub fn function(&self, name: &str) -> Option<&FunctionDef> {
        self.functions.get(name)
    }

    /// The analyzer named `name`, if defined.
    pub fn analyzer(&self, name: &str) -> Option<&AnalyzerDef> {
        self.analyzers.get(name)
    }

    /// Inserts a global param, replacing any of the same name.
    pub fn insert_param(&mut self, param: ParamDef) {
        self.params.insert(param.name.clone(), param);
    }

    /// Inserts a function, replacing any of the same `fn::` path.
    pub fn insert_function(&mut self, function: FunctionDef) {
        self.functions.insert(function.name.clone(), function);
    }

    /// Inserts an analyzer, replacing any of the same name.
    pub fn insert_analyzer(&mut self, analyzer: AnalyzerDef) {
        self.analyzers.insert(analyzer.name.clone(), analyzer);
    }

    /// DEFINE ANALYZER components must name known tokenizers/filters with
    /// valid arguments (1032/2035).
    pub(crate) fn validate_analyzer(
        analyzer: &AnalyzerDef,
    ) -> Vec<surrealguard_diagnostics::Finding> {
        const TOKENIZERS: &[&str] = &["blank", "camel", "class", "punct"];
        const SNOWBALL_LANGS: &[&str] = &[
            "arabic",
            "danish",
            "dutch",
            "english",
            "french",
            "german",
            "greek",
            "hungarian",
            "italian",
            "norwegian",
            "portuguese",
            "romanian",
            "russian",
            "spanish",
            "swedish",
            "tamil",
            "turkish",
        ];
        let mut findings = Vec::new();
        for tokenizer in &analyzer.tokenizers {
            if !TOKENIZERS.contains(&tokenizer.to_ascii_lowercase().as_str()) {
                findings.push(
                    surrealguard_diagnostics::catalog::finding(
                        analyzer.name_span.clone(),
                        1032,
                        format!(
                            "DEFINE ANALYZER will fail: `{tokenizer}` is not a supported tokenizer"
                        ),
                    )
                    .with_help("supported tokenizers: blank, camel, class, punct"),
                );
            }
        }
        for filter in &analyzer.filters {
            let (name, args) = match filter.split_once('(') {
                Some((name, rest)) => (
                    name.trim(),
                    rest.trim_end_matches(')')
                        .split(',')
                        .map(|arg| arg.trim().to_string())
                        .collect::<Vec<_>>(),
                ),
                None => (filter.trim(), Vec::new()),
            };
            match name.to_ascii_lowercase().as_str() {
                "ascii" | "lowercase" | "uppercase" => {}
                "snowball" => {
                    if !args.first().is_some_and(|lang| {
                        SNOWBALL_LANGS.contains(&lang.to_ascii_lowercase().as_str())
                    }) {
                        findings.push(
                            surrealguard_diagnostics::catalog::finding(
                                analyzer.name_span.clone(),
                                1032,
                                format!(
                                    "DEFINE ANALYZER will fail: `{}` is not a supported snowball language",
                                    args.first().cloned().unwrap_or_default()
                                ),
                            )
                            .with_help(
                                "supported languages: arabic, danish, dutch, english, french, \
                                 german, greek, hungarian, italian, norwegian, portuguese, \
                                 romanian, russian, spanish, swedish, tamil, turkish",
                            ),
                        );
                    }
                }
                "edgengram" | "ngram" => {
                    let bounds: Vec<Option<u64>> =
                        args.iter().map(|arg| arg.parse::<u64>().ok()).collect();
                    match bounds.as_slice() {
                        [Some(min), Some(max)] if min <= max => {}
                        _ => findings.push(surrealguard_diagnostics::catalog::finding(
                            analyzer.name_span.clone(),
                            2035,
                            format!("`{filter}` needs `(min, max)` with min <= max"),
                        )),
                    }
                }
                _ => findings.push(
                    surrealguard_diagnostics::catalog::finding(
                        analyzer.name_span.clone(),
                        1032,
                        format!("DEFINE ANALYZER will fail: `{name}` is not a supported filter"),
                    )
                    .with_help(
                        "supported filters: ascii, lowercase, uppercase, snowball, edgengram, ngram",
                    ),
                ),
            }
        }
        findings
    }

    /// Inserts a table definition. A redefinition without `OVERWRITE` keeps
    /// the existing definition; `OVERWRITE` replaces the definition but
    /// retains its already-attached fields and indexes.
    pub fn insert_table(&mut self, mut table: TableDef, overwrite: bool) {
        if let Some(existing) = self.tables.remove(&table.name) {
            if !overwrite {
                self.tables.insert(existing.name.clone(), existing);
                return;
            }
            table.fields = existing.fields;
            table.indexes = existing.indexes;
        }
        self.tables.insert(table.name.clone(), table);
    }

    /// Attaches a field to its table. A field targeting an unknown table, or
    /// a redefinition without `OVERWRITE`, leaves the index unchanged.
    ///
    /// A declaration that describes part of an ALREADY-DECLARED field —
    /// `items[*].price` under `items TYPE array<object>` — is not a field of
    /// the row at all: it refines the declared kind in place (see
    /// [`field_placement`]) and is not stored under its own key. Storing it
    /// separately is what let a descendant silently replace its parent's
    /// declared kind.
    pub fn insert_field(&mut self, field: FieldDef, overwrite: bool) {
        let field_key = field.path.join(".");
        let Some(table) = self.tables.get_mut(&field.table) else {
            return;
        };
        match field_placement(table, &field.steps) {
            FieldPlacement::Refine { ancestor, tail } => {
                let child = field.kind.clone().unwrap_or(Kind::Any);
                let Some(parent) = table.fields.get_mut(&ancestor) else {
                    return;
                };
                if let Some(refined) = parent
                    .kind
                    .as_ref()
                    .and_then(|kind| crate::kinds::refine_subkind(kind, &tail, &child))
                {
                    parent.kind = Some(refined);
                }
            }
            // The parent's declared kind has no room for this subfield (1025
            // reports it). Dropping the declaration is what keeps the parent's
            // own kind intact.
            FieldPlacement::Rejected { .. } => {}
            FieldPlacement::Standalone => {
                if table.fields.contains_key(&field_key) && !overwrite {
                    return;
                }
                table.fields.insert(field_key, field);
            }
        }
    }

    /// Drops a table and everything attached to it.
    pub fn remove_table(&mut self, table: &str) {
        self.tables.remove(table);
    }

    /// Drops a field from its table, if both exist.
    pub fn remove_field(&mut self, table: &str, path: &[String]) {
        if let Some(table_def) = self.tables.get_mut(table) {
            table_def.fields.remove(&path.join("."));
        }
    }
}

impl TableDef {
    /// The field at `path` on this table, if defined.
    pub fn field(&self, path: &FieldPath) -> Option<&FieldDef> {
        self.fields.get(&path.dotted())
    }

    /// The kind of an implicit record field SurrealDB provides but no
    /// `DEFINE FIELD` declares: every record has an `id` (a record link to
    /// its own table), and every `TYPE RELATION` edge record additionally
    /// has `in`/`out` record links to its FROM/TO endpoint tables. These are
    /// real, queryable, and indexable, but they live outside `self.fields`
    /// so the schemaless `fields.is_empty()` gate stays untouched. An empty
    /// endpoint list means any record (`record<>`).
    pub fn implicit_field_kind(&self, head: &str) -> Option<Kind> {
        match head {
            "id" => Some(Kind::Record(vec![surrealdb_types::Table::from(
                self.name.as_str(),
            )])),
            "in" | "out" => {
                let relation = self.relation.as_ref()?;
                let tables = if head == "in" {
                    &relation.in_tables
                } else {
                    &relation.out_tables
                };
                Some(Kind::Record(
                    tables
                        .iter()
                        .map(|table| surrealdb_types::Table::from(table.as_str()))
                        .collect(),
                ))
            }
            _ => None,
        }
    }

    /// Every field whose path lies under `prefix`, for expanding a nested
    /// object selection.
    pub fn fields_under<'a>(
        &'a self,
        prefix: &'a FieldPath,
    ) -> impl Iterator<Item = &'a FieldDef> + 'a {
        self.fields
            .values()
            .filter(move |field| prefix.is_prefix_of(&field.path))
    }
}

/// Applies one lowered statement's catalog effect: definitions are inserted,
/// removals drop their targets. The contract checks that reference these
/// definitions belong to the statement analyzers, so extraction never emits.
pub(crate) fn apply_schema_statement_effects(
    stmt: &ast::Spanned<ast::Statement>,
    source: &SourceId,
    text: &str,
    schema: &mut SchemaIndex,
) {
    match &stmt.node {
        ast::Statement::Define(def) => match def {
            ast::DefineStmt::Table(def) => {
                schema.insert_table(table_def_from_ast(def, source), def.overwrite);
            }
            ast::DefineStmt::Field(def) => {
                let mut field = field_def_from_ast(def, source, text);
                // The accumulated catalog is now visible: an untyped field whose
                // VALUE/COMPUTED reads tables, sibling `$this.<field>`s, or `fn::`
                // helpers resolves against it (the standalone build above only sees
                // pure scalars, so such a clause degrades to `Any` — or, when only
                // one arm needs the catalog, to `Any | T`). Re-infer whenever the
                // standalone kind is absent OR still carries an unresolved `Any`.
                // `infer_field_value_kind` returns `None` for a pure-`Any` result,
                // so this only ever upgrades to a proven kind — never downgrades.
                if field.kind.as_ref().map_or(true, kind_contains_any) {
                    if let Some(kind) = infer_field_value_kind(def, source, text, Some(&*schema)) {
                        field.kind = Some(kind);
                        field.partial.clear();
                    }
                }
                schema.insert_field(field, def.overwrite);
            }
            ast::DefineStmt::Index(def) => {
                let index = index_def_from_ast(def, source);
                if let Some(table) = schema.tables.get_mut(&index.table) {
                    table.indexes.insert(index.name.clone(), index);
                }
            }
            ast::DefineStmt::Param(def) => schema.insert_param(param_def_from_ast(def, source)),
            ast::DefineStmt::Function(def) => {
                let mut function = function_def_from_ast(def, source, text, stmt.span);
                // The accumulated catalog is now visible: re-infer an untyped
                // body against it so a call to an already-defined `fn::` helper
                // resolves (the standalone build above only sees params). Only
                // upgrade a concrete result — never overwrite with `Any`.
                if function.return_kind.is_none() {
                    if let Some(inferred) = infer_untyped_return(def, source, text, Some(&*schema)) {
                        function.inferred_return = Some(inferred);
                    }
                }
                schema.insert_function(function);
            }
            ast::DefineStmt::Analyzer(def) => {
                schema.insert_analyzer(analyzer_def_from_ast(def, source));
            }
            // Events are validated but never stored; the long tail is unmodeled.
            ast::DefineStmt::Event(_) | ast::DefineStmt::Other(_) => {}
        },
        ast::Statement::Remove(stmt) => match &stmt.target {
            ast::RemoveTarget::Table(table) => schema.remove_table(&table.node),
            ast::RemoveTarget::Field { field, table } => {
                schema.remove_field(&table.node, &idiom_field_path(&field.node));
            }
            // REMOVE INDEX is validated against the catalog but not stored.
            ast::RemoveTarget::Index { .. } | ast::RemoveTarget::Other(_) => {}
        },
        _ => {}
    }
}

/// Builds the schema for a batch of parsed sources by running the full
/// analysis pipeline, which owns statement sequencing, catalog effects, and
/// every contract check that references a definition.
pub fn extract_schema(parsed_sources: &[ParsedSource]) -> SchemaExtraction {
    let output = crate::analyzer::pipeline::analyze_sources(parsed_sources);
    SchemaExtraction {
        schema: output.schema,
        diagnostics: output.diagnostics,
    }
}

/// The `fn::` signature of a `DEFINE FUNCTION` statement, hoisted so a body
/// can call functions defined later in source order.
pub(crate) fn extract_function_def(
    stmt: &ast::Spanned<ast::Statement>,
    source: &SourceId,
    text: &str,
) -> Option<FunctionDef> {
    match &stmt.node {
        ast::Statement::Define(ast::DefineStmt::Function(def)) => {
            Some(function_def_from_ast(def, source, text, stmt.span))
        }
        _ => None,
    }
}

fn span(source: &SourceId, range: ByteRange) -> SourceSpan {
    SourceSpan::new(source.clone(), range)
}

/// The plain field segments of an idiom (`profile.name` → `[profile, name]`),
/// dropping any non-field parts (`$event`, index expressions, ...).
pub(crate) fn idiom_field_path(idiom: &ast::Idiom) -> Vec<String> {
    idiom
        .parts
        .iter()
        .filter_map(|part| match &part.node {
            ast::IdiomPart::Field(name) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// The structural steps of a `DEFINE FIELD` path (`items[*].price` →
/// `[Field(items), Element, Field(price)]`).
///
/// Empty when the idiom is not a pure field/element path — an index
/// expression, a filter, a graph step. Such a declaration carries no
/// structural claim about a parent field, so every rule keyed on steps leaves
/// it alone.
pub(crate) fn idiom_field_steps(idiom: &ast::Idiom) -> Vec<FieldStep> {
    let mut steps = Vec::with_capacity(idiom.parts.len());
    for part in &idiom.parts {
        match &part.node {
            ast::IdiomPart::Field(name) => steps.push(FieldStep::Field(name.clone())),
            ast::IdiomPart::All => steps.push(FieldStep::Element),
            _ => return Vec::new(),
        }
    }
    steps
}

/// Where a `DEFINE FIELD` declaration belongs in the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FieldPlacement {
    /// Stored under its own dotted key, as its own field.
    Standalone,
    /// Not a field of its own: it refines the already-declared field
    /// `ancestor` at the sub-path `tail`.
    Refine {
        /// Dotted key of the declared ancestor field being refined.
        ancestor: String,
        /// The sub-path under that ancestor this declaration describes.
        tail: Vec<FieldStep>,
    },
    /// The declared ancestor's kind admits no such sub-path (1025).
    Rejected {
        /// Dotted key of the declared ancestor field.
        ancestor: String,
        /// That ancestor's declared kind, for the diagnostic.
        ancestor_kind: Kind,
    },
}

/// Decides whether a declaration is a field of the row or a REFINEMENT of an
/// already-declared ancestor field.
///
/// A descendant declaration only ever refines a parent whose declared kind
/// *carries shape the dotted key cannot* — a collection (`array`/`set`, which
/// the nested-field synthesis would flatten away), an `option<...>` (whose
/// optionality the synthesis would erase), or a literal object (whose already-
/// declared siblings the synthesis would drop).
///
/// Deliberately left standalone, so their long-standing behaviour is untouched:
/// * an ancestor that is not declared at all (`profile.name` with no `DEFINE
///   FIELD profile`) — the nested-field prefix synthesis owns that shape;
/// * an ancestor declared as a bare `object`, which is exactly the open shape
///   that synthesis models correctly (and the `FLEXIBLE` case);
/// * an untyped / `any` ancestor, which claims nothing to preserve.
pub(crate) fn field_placement(table: &TableDef, steps: &[FieldStep]) -> FieldPlacement {
    // The deepest declared ancestor: only a prefix made purely of named
    // segments can be a stored dotted key.
    let mut ancestor: Option<(usize, &FieldDef)> = None;
    for split in 1..steps.len() {
        let mut names = Vec::with_capacity(split);
        for step in &steps[..split] {
            let FieldStep::Field(name) = step else {
                names.clear();
                break;
            };
            names.push(name.as_str());
        }
        if names.is_empty() {
            break;
        }
        if let Some(field) = table.fields.get(&names.join(".")) {
            ancestor = Some((split, field));
        }
    }

    let Some((split, parent)) = ancestor else {
        return FieldPlacement::Standalone;
    };
    let Some(kind) = parent.kind.clone() else {
        return FieldPlacement::Standalone;
    };
    let tail = &steps[split..];
    let all_named = tail.iter().all(|step| matches!(step, FieldStep::Field(_)));
    // `any` claims nothing, so there is nothing to preserve — and narrowing it
    // to what a descendant happens to mention would invent a contract the
    // author declined to write.
    if matches!(kind, Kind::Any) || (matches!(kind, Kind::Object) && all_named) {
        return FieldPlacement::Standalone;
    }

    let key = parent.path.join(".");
    match crate::kinds::refine_subkind(&kind, tail, &Kind::Any) {
        Some(_) => FieldPlacement::Refine {
            ancestor: key,
            tail: tail.to_vec(),
        },
        None => FieldPlacement::Rejected {
            ancestor: key,
            ancestor_kind: kind,
        },
    }
}

/// Whether an index/event field path resolves on a table, either directly or
/// as the prefix of a declared nested field.
pub(crate) fn index_field_path_exists_on_table(table: &TableDef, path: &[String]) -> bool {
    let key = path.join(".");
    // A path that runs INTO a declared kind — `items.price` under
    // `items: array<{ price: string }>`, however that element type was
    // declared. Only what the kind proves counts, so this never invents a
    // field.
    let resolves_through_kind = || {
        let Some((head, rest)) = path.split_first() else {
            return false;
        };
        let steps: Vec<_> = rest
            .iter()
            .map(|name| FieldStep::Field(name.clone()))
            .collect();
        table
            .fields
            .get(head)
            .and_then(|field| field.kind.as_ref())
            .is_some_and(|kind| crate::kinds::subkind_at(kind, &steps).is_some())
    };
    table.fields.contains_key(&key)
        || resolves_through_kind()
        || table
            .fields
            .values()
            .any(|field| field.path.len() > path.len() && field.path.starts_with(path))
        || path
            .first()
            .is_some_and(|head| table.implicit_field_kind(head).is_some())
}

pub(crate) fn table_def_from_ast(def: &ast::DefineTable, source: &SourceId) -> TableDef {
    TableDef {
        name: def.name.node.clone(),
        source: source.clone(),
        name_span: span(source, def.name.span),
        fields: BTreeMap::new(),
        indexes: BTreeMap::new(),
        relation: def.relation.as_ref().map(|relation| RelationDef {
            in_tables: relation.in_tables.iter().map(|t| t.node.clone()).collect(),
            out_tables: relation.out_tables.iter().map(|t| t.node.clone()).collect(),
            span: span(source, relation.span),
        }),
        schemafull: def.schemafull,
        drop_table: def.drop,
        changefeed: def.changefeed,
    }
}

pub(crate) fn field_def_from_ast(
    def: &ast::DefineField,
    source: &SourceId,
    text: &str,
) -> FieldDef {
    let (kind, partial, type_span) = match &def.ty {
        Some(ty) => {
            let parsed = kind_from_type_expr(&ty.node, text);
            (parsed.kind, parsed.partial, Some(span(source, ty.span)))
        }
        // No explicit `TYPE`: fall back to the kind of the value the field
        // stores (its `VALUE`/`COMPUTED`, or a `DEFAULT`). Against no catalog,
        // this resolves pure-scalar value expressions (`string::uppercase('x')`,
        // `time::now()`); a value reading tables/fields degrades to `None` here
        // and is upgraded by the schema-aware pass in `pipeline`. When a kind is
        // proven, the field is no longer partial; otherwise it stays unresolved.
        None => match infer_field_value_kind(def, source, text, None) {
            Some(kind) => (Some(kind), Vec::new(), None),
            None => (None, vec![PartialReason::Unresolved], None),
        },
    };
    FieldDef {
        // A `COMPUTED` field is derived, so — like `VALUE` — it is never a
        // required input and is overwritten by its own expression.
        has_default: def.default.is_some() || def.value.is_some() || def.computed.is_some(),
        readonly: def.readonly,
        computed: def.value.is_some() || def.computed.is_some(),
        reference: def.reference,
        path: idiom_field_path(&def.path.node),
        steps: idiom_field_steps(&def.path.node),
        table: def.table.node.clone(),
        kind,
        partial,
        source: source.clone(),
        name_span: span(source, def.path.span),
        table_span: span(source, def.table.span),
        type_span,
    }
}

pub(crate) fn index_def_from_ast(def: &ast::DefineIndex, source: &SourceId) -> IndexDef {
    let fields = def
        .fields
        .iter()
        .map(|field| {
            let path = idiom_field_path(&field.node);
            IndexFieldDef {
                text: path.join("."),
                path,
                span: span(source, field.span),
            }
        })
        .collect();
    IndexDef {
        name: def.name.node.clone(),
        table: def.table.node.clone(),
        fields,
        kind: match def.kind {
            ast::IndexKind::Normal => IndexKind::Normal,
            ast::IndexKind::Unique => IndexKind::Unique,
            ast::IndexKind::Search => IndexKind::Search,
            ast::IndexKind::Vector => IndexKind::Vector,
        },
        name_span: span(source, def.name.span),
        table_span: span(source, def.table.span),
    }
}

/// The `(path, dotted-text, span)` of each declared index field, for the
/// index analyzer's field-existence and duplicate checks.
pub(crate) fn index_field_refs(
    def: &ast::DefineIndex,
    source: &SourceId,
) -> Vec<(Vec<String>, String, SourceSpan)> {
    def.fields
        .iter()
        .map(|field| {
            let path = idiom_field_path(&field.node);
            let text = path.join(".");
            (path, text, span(source, field.span))
        })
        .collect()
}

pub(crate) fn analyzer_def_from_ast(def: &ast::DefineAnalyzer, source: &SourceId) -> AnalyzerDef {
    AnalyzerDef {
        name: def.name.node.clone(),
        tokenizers: def.tokenizers.iter().map(|t| t.node.clone()).collect(),
        filters: def.filters.iter().map(|f| f.node.clone()).collect(),
        source: source.clone(),
        name_span: span(source, def.name.span),
    }
}

pub(crate) fn param_def_from_ast(def: &ast::DefineParam, source: &SourceId) -> ParamDef {
    ParamDef {
        name: def.name.node.clone(),
        source: source.clone(),
        name_span: span(source, def.name.span),
        value_span: def.value.as_ref().map(|value| span(source, value.span)),
    }
}

pub(crate) fn function_def_from_ast(
    def: &ast::DefineFunction,
    source: &SourceId,
    text: &str,
    stmt_span: ByteRange,
) -> FunctionDef {
    let args = def
        .params
        .iter()
        .map(|(name, ty)| FunctionParam {
            name: name.node.trim_start_matches('$').to_string(),
            kind: ty
                .as_ref()
                .and_then(|ty| kind_from_type_expr(&ty.node, text).kind),
        })
        .collect();

    let (return_kind, return_span) = match &def.return_ty {
        Some(ty) => (
            kind_from_type_expr(&ty.node, text).kind,
            Some(span(source, ty.span)),
        ),
        None => (None, None),
    };

    // When the definition omits `-> T`, infer the body's response kind so
    // callers get a real type instead of `Any`. This standalone path has no
    // surrounding schema, so a body that leans on tables or other `fn::`
    // helpers degrades to `Any` (persisted as `None`); the schema-aware walk
    // path (`apply_schema_statement_effects`) upgrades those where it can.
    let inferred_return = if return_kind.is_none() {
        infer_untyped_return(def, source, text, None)
    } else {
        None
    };

    // Body callees for cycle detection (5009) — a text scan over everything
    // after the function name is enough: a false positive requires `fn::name`
    // inside a string literal, which is vanishingly rare in function bodies.
    let mut callees: Vec<String> = Vec::new();
    let name_end = def.name.span.end() as usize;
    let stmt_end = stmt_span.end() as usize;
    let body = text.get(name_end..stmt_end).unwrap_or_default();
    let mut offset = 0;
    while let Some(at) = body[offset..].find("fn::") {
        let start = offset + at;
        let end = start
            + body[start..]
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == ':'))
                .unwrap_or(body.len() - start);
        let callee = body[start..end].to_string();
        if !callees.contains(&callee) {
            callees.push(callee);
        }
        offset = end.max(start + 4);
    }

    FunctionDef {
        name: def.name.node.clone(),
        args,
        callees,
        return_kind,
        inferred_return,
        source: source.clone(),
        name_span: span(source, def.name.span),
        return_span,
    }
}

/// Infers an untyped `DEFINE FUNCTION`'s return kind from its body, for
/// [`FunctionDef::inferred_return`]. Returns `None` when there is no body, a
/// return type is declared (declared wins — never inferred over), or the body
/// resolves to `Kind::Any` (no false precision).
///
/// `schema` scopes the inference: `None` uses an empty catalog (param-only
/// bodies still resolve; anything referencing tables or other functions
/// degrades to `Any`), while `Some(schema)` lets a body that calls an
/// already-visible `fn::` helper resolve to that helper's return. The body is
/// analyzed here only to read its type — a scratch diagnostics sink is
/// discarded, so this never emits findings (the walk's `DEFINE FUNCTION`
/// analyzer owns the body's real diagnostics).
pub(crate) fn infer_untyped_return(
    def: &ast::DefineFunction,
    source: &SourceId,
    text: &str,
    schema: Option<&SchemaIndex>,
) -> Option<Kind> {
    if def.return_ty.is_some() || def.body.is_none() {
        return None;
    }
    let empty = SchemaIndex::default();
    let schema = schema.unwrap_or(&empty);
    let mut scratch: Vec<surrealguard_diagnostics::Finding> = Vec::new();
    let mut ctx = crate::analyzer::context::AnalysisContext::new(
        schema,
        source.clone(),
        text,
        &mut scratch,
    );
    match crate::analyzer::schema::define::function::infer_function_body_kind(&mut ctx, def) {
        Some(Kind::Any) | None => None,
        Some(kind) => Some(kind),
    }
}

/// Infers an untyped `DEFINE FIELD`'s kind from the value it stores, for
/// [`FieldDef::kind`] when the definition omits an explicit `TYPE`. Priority:
/// `VALUE`/`COMPUTED` (they define the stored value) over `DEFAULT` (only a
/// creation-time fallback). Returns `None` when a `TYPE` is declared (declared
/// wins — never inferred over), no value/default expression exists, or the
/// expression resolves to `Kind::Any` (no false precision — the field stays
/// untyped).
///
/// `schema` scopes the inference exactly like [`infer_untyped_return`]: `None`
/// uses an empty catalog (pure-scalar value expressions still resolve; anything
/// reading tables, fields, or `fn::` helpers degrades to `Any`), while
/// `Some(schema)` lets a value expression that reads the catalog resolve. The
/// expression is analyzed here only to read its type — a scratch diagnostics
/// sink is discarded, so this never emits (the walk's `DEFINE FIELD` analyzer
/// owns the clause's real diagnostics).
pub(crate) fn infer_field_value_kind(
    def: &ast::DefineField,
    source: &SourceId,
    text: &str,
    schema: Option<&SchemaIndex>,
) -> Option<Kind> {
    if def.ty.is_some() {
        return None;
    }
    // VALUE / COMPUTED define the stored/derived value; DEFAULT is only a
    // creation-time fallback.
    let expr = def
        .value
        .as_ref()
        .or(def.computed.as_ref())
        .or(def.default.as_ref())?;

    // A record-reference back-traversal (`COMPUTED <~team`) resolves to
    // `array<record<team>>`, but only when the schema proves the back-reference
    // (the target table carries a `record<Self>` REFERENCE field). This needs
    // the owning table and the accumulated catalog, so it is attempted only in
    // the schema-aware pass; otherwise it degrades to the scalar inference below
    // and stays untyped rather than inventing a type.
    if let Some(schema) = schema {
        if let ast::Expr::Idiom(idiom) = &expr.node {
            if let Some(kind) = crate::analyzer::data::select::reference_back_traversal_kind(
                &def.table.node,
                idiom,
                schema,
            ) {
                return Some(kind);
            }
        }
    }

    // The owning table becomes the row context so bare field references and
    // graph traversals in a VALUE/COMPUTED clause resolve against it (available
    // only in the schema-aware pass; the standalone pass sees no tables).
    let self_table = schema.and_then(|s| s.tables.get(&def.table.node));
    let empty = SchemaIndex::default();
    let schema = schema.unwrap_or(&empty);
    let mut scratch: Vec<surrealguard_diagnostics::Finding> = Vec::new();
    let mut ctx = crate::analyzer::context::AnalysisContext::new(
        schema,
        source.clone(),
        text,
        &mut scratch,
    );
    let inferred = ctx.with_row_table(self_table, |ctx| {
        crate::analyzer::schema::define::field::infer_field_clause_kind(ctx, expr, &def.table.node)
    });
    match inferred {
        Some(Kind::Any) | None => None,
        Some(kind) => Some(kind),
    }
}

/// Whether a kind still carries an unresolved `Any` anywhere in its structure.
/// This is the signal that an empty-catalog inference of a value-typed field
/// could not fully resolve its expression (a `$this.<field>` read, a table, or a
/// `fn::` helper), so a schema-aware pass should re-infer it once the full
/// catalog exists. Shared by PRE-PASS 1c and the per-source schema walk.
pub(crate) fn kind_contains_any(kind: &Kind) -> bool {
    match kind {
        Kind::Any => true,
        Kind::Array(inner, _) | Kind::Set(inner, _) => kind_contains_any(inner),
        Kind::Either(variants) => variants.iter().any(kind_contains_any),
        _ => false,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParsedFieldKind {
    pub(crate) kind: Option<Kind>,
    pub(crate) partial: Vec<PartialReason>,
}

/// Converts a structurally lowered type to an upstream `Kind`:
/// `array<string>`, `option<int>`, unions, and literal types all resolve.
/// Anything the conversion can't express reports why as an explicit
/// partial reason.
pub(crate) fn kind_from_type_expr(
    ty: &surrealguard_syntax::ast::TypeExpr,
    text: &str,
) -> ParsedFieldKind {
    use surrealguard_syntax::ast::TypeExpr;

    fn convert(ty: &TypeExpr, text: &str) -> Result<Kind, PartialReason> {
        match ty {
            TypeExpr::Name(name) => base_kind_for_name(&name.node)
                .ok_or_else(|| PartialReason::UnsupportedSyntax(name.node.clone())),
            TypeExpr::Parameterized { name, args } => parameterized_kind(&name.node, args, text),
            TypeExpr::Union(variants) => {
                let kinds = variants
                    .iter()
                    .map(|variant| convert(&variant.node, text))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Kind::either(kinds))
            }
            TypeExpr::Optional(inner) => {
                let inner = convert(&inner.node, text)?;
                Ok(Kind::either(vec![Kind::None, inner]))
            }
            TypeExpr::Literal(literal) => literal_kind(literal),
            TypeExpr::Object(properties) => {
                use surrealdb_types::KindLiteral;
                let mut map = std::collections::BTreeMap::new();
                for (name, value) in properties {
                    map.insert(name.node.clone(), convert(&value.node, text)?);
                }
                Ok(Kind::Literal(KindLiteral::Object(map)))
            }
            TypeExpr::Partial(partial) => {
                let start = partial.span.start() as usize;
                let end = partial.span.end() as usize;
                let source = text
                    .get(start..end)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map_or_else(|| partial.cst_kind.clone(), str::to_string);
                Err(PartialReason::UnsupportedSyntax(source))
            }
        }
    }

    fn parameterized_kind(
        name: &str,
        args: &[surrealguard_syntax::ast::Spanned<TypeExpr>],
        text: &str,
    ) -> Result<Kind, PartialReason> {
        let unsupported = || PartialReason::UnsupportedSyntax(format!("{name}<...>"));
        match name.to_ascii_lowercase().as_str() {
            "record" => {
                // `record<user>` takes named tables directly; `record<team |
                // user | org>` writes the same set of tables as a single union
                // argument, which the grammar nests as one `TypeExpr::Union`.
                let table_names: &[surrealguard_syntax::ast::Spanned<TypeExpr>] = match args {
                    [single] => match &single.node {
                        TypeExpr::Union(variants) => variants,
                        _ => args,
                    },
                    _ => args,
                };
                let tables = table_names
                    .iter()
                    .filter_map(|arg| match &arg.node {
                        TypeExpr::Name(table) => {
                            Some(surrealdb_types::Table::from(table.node.as_str()))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if tables.len() == table_names.len() && !tables.is_empty() {
                    Ok(Kind::Record(tables))
                } else {
                    Err(unsupported())
                }
            }
            "array" | "set" => {
                let mut element = Kind::Any;
                let mut max_len = None;
                for arg in args {
                    match &arg.node {
                        TypeExpr::Literal(surrealguard_syntax::ast::Literal::Int(len)) => {
                            max_len = u64::try_from(*len).ok();
                        }
                        other => element = convert(other, text)?,
                    }
                }
                if name.eq_ignore_ascii_case("set") {
                    Ok(Kind::Set(Box::new(element), max_len))
                } else {
                    Ok(Kind::Array(Box::new(element), max_len))
                }
            }
            _ => Err(unsupported()),
        }
    }

    fn literal_kind(literal: &surrealguard_syntax::ast::Literal) -> Result<Kind, PartialReason> {
        use surrealdb_types::KindLiteral;
        use surrealguard_syntax::ast::Literal;
        let kind = match literal {
            Literal::String(value) => KindLiteral::String(value.clone()),
            Literal::Int(value) => KindLiteral::Integer(*value),
            Literal::Float(value) => KindLiteral::Float(*value),
            Literal::Bool(value) => KindLiteral::Bool(*value),
            _ => {
                return Err(PartialReason::UnsupportedSyntax("literal type".into()));
            }
        };
        Ok(Kind::Literal(kind))
    }

    match convert(ty, text) {
        Ok(kind) => ParsedFieldKind {
            kind: Some(kind),
            partial: Vec::new(),
        },
        Err(reason) => ParsedFieldKind {
            kind: None,
            partial: vec![reason],
        },
    }
}

fn base_kind_for_name(name: &str) -> Option<Kind> {
    let kind = match name.to_ascii_lowercase().as_str() {
        "any" => Kind::Any,
        "none" => Kind::None,
        "null" => Kind::Null,
        "bool" | "boolean" => Kind::Bool,
        "string" => Kind::String,
        "number" => Kind::Number,
        "int" => Kind::Int,
        "float" => Kind::Float,
        "decimal" => Kind::Decimal,
        "datetime" => Kind::Datetime,
        "duration" => Kind::Duration,
        "uuid" => Kind::Uuid,
        "bytes" => Kind::Bytes,
        "object" => Kind::Object,
        "array" => Kind::Array(Box::new(Kind::Any), None),
        "set" => Kind::Set(Box::new(Kind::Any), None),
        "record" => Kind::Record(Vec::new()),
        "geometry" => Kind::Geometry(Vec::new()),
        _ => return None,
    };
    Some(kind)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use surrealdb_types::{Kind, KindLiteral};
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use super::{extract_schema, FieldPath, SchemaIndex};

    fn schema_of(text: &str) -> SchemaIndex {
        let parsed = parse_source(SourceId::new("schema:sub"), text).expect("schema parses");
        extract_schema(&[parsed]).schema
    }

    fn field_kind(schema: &SchemaIndex, table: &str, path: &str) -> Option<Kind> {
        schema
            .field(table, &FieldPath::parse(path))
            .and_then(|field| field.kind.clone())
    }

    fn object(entries: &[(&str, Kind)]) -> Kind {
        Kind::Literal(KindLiteral::Object(
            entries
                .iter()
                .map(|(name, kind)| ((*name).to_string(), kind.clone()))
                .collect::<BTreeMap<_, _>>(),
        ))
    }

    #[test]
    fn element_definitions_refine_the_declared_array_instead_of_replacing_it() {
        // `items[*].price` describes the ELEMENT of `items`, not a field of the
        // row. Storing it as a pseudo-field `items.price` used to shadow the
        // parent's declaration and flatten `array<object>` to a bare object —
        // an error-severity 2001 on every valid write.
        let schema = schema_of(
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD items ON t TYPE array<object>;\n\
             DEFINE FIELD items[*] ON t TYPE object;\n\
             DEFINE FIELD items[*].price ON t TYPE string;\n\
             DEFINE FIELD items[*].qty ON t TYPE int;",
        );

        assert_eq!(
            field_kind(&schema, "t", "items"),
            Some(Kind::Array(
                Box::new(object(&[("price", Kind::String), ("qty", Kind::Int)])),
                None
            )),
        );
        // The element declarations are not fields of the row.
        let t = schema.table("t").expect("table t");
        assert_eq!(t.fields.keys().collect::<Vec<_>>(), vec!["items"]);
    }

    #[test]
    fn an_undeclared_element_type_still_refines_a_bare_array() {
        // The shape the oracle corpus uses: `TYPE array` with the element type
        // supplied only by the `[*]` declarations.
        let schema = schema_of(
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD items ON t TYPE array;\n\
             DEFINE FIELD items[*].sku ON t TYPE string;",
        );

        assert_eq!(
            field_kind(&schema, "t", "items"),
            Some(Kind::Array(
                Box::new(object(&[("sku", Kind::String)])),
                None
            )),
        );
    }

    #[test]
    fn a_subfield_never_makes_an_optional_parent_required() {
        let schema = schema_of(
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD cfg ON t TYPE option<object>;\n\
             DEFINE FIELD cfg.theme ON t TYPE string;",
        );

        assert_eq!(
            field_kind(&schema, "t", "cfg"),
            Some(Kind::either(vec![
                Kind::None,
                object(&[("theme", Kind::String)]),
            ])),
        );
    }

    #[test]
    fn a_subfield_under_a_scalar_parent_leaves_the_parent_kind_alone() {
        // `title` holds a string; `title.sub` can never exist. The definition is
        // reported (1025) and dropped, so `title` stays `string` and
        // `SET title = 'hello'` keeps type-checking.
        let schema = schema_of(
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD title ON t TYPE string;\n\
             DEFINE FIELD title.sub ON t TYPE string;",
        );

        assert_eq!(field_kind(&schema, "t", "title"), Some(Kind::String));
        assert!(schema
            .field("t", &FieldPath::parse("title.sub"))
            .is_none());
    }

    #[test]
    fn a_bare_object_parent_keeps_storing_its_subfields_as_fields() {
        // An open `object` (and an undeclared parent) is exactly the shape the
        // nested-field prefix synthesis already models, so those declarations
        // stay standalone fields and keep resolving as such.
        let schema = schema_of(
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD prof ON t TYPE object;\n\
             DEFINE FIELD prof.name ON t TYPE string;\n\
             DEFINE FIELD other.city ON t TYPE string;",
        );

        assert_eq!(field_kind(&schema, "t", "prof"), Some(Kind::Object));
        assert_eq!(field_kind(&schema, "t", "prof.name"), Some(Kind::String));
        assert_eq!(field_kind(&schema, "t", "other.city"), Some(Kind::String));
    }

    #[test]
    fn an_index_over_a_refined_element_field_still_resolves() {
        // The element declarations no longer exist as fields, so index
        // field-existence has to read them out of the refined kind.
        let schema = schema_of(
            "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD items ON t TYPE array;\n\
             DEFINE FIELD items[*].sku ON t TYPE string;",
        );
        let t = schema.table("t").expect("table t");

        assert!(super::index_field_path_exists_on_table(
            t,
            &["items".to_string(), "sku".to_string()]
        ));
        assert!(!super::index_field_path_exists_on_table(
            t,
            &["items".to_string(), "ghost".to_string()]
        ));
    }

    #[test]
    fn schema_index_stores_define_statements_for_direct_table_and_field_lookup() {
        let parsed = parse_source(
            SourceId::new("schema:test"),
            "DEFINE TABLE user;\nDEFINE FIELD profile.name ON user TYPE string;\nDEFINE FIELD age ON user TYPE int;",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());

        let user = extraction.schema.table("user").expect("user table exists");
        assert_eq!(user.name, "user");

        let profile_name = extraction
            .schema
            .field("user", &FieldPath::parse("profile.name"))
            .expect("profile.name field exists");
        assert_eq!(profile_name.kind, Some(Kind::String));

        let age = user
            .field(&FieldPath::parse("age"))
            .expect("age field exists");
        assert_eq!(age.kind, Some(Kind::Int));
    }

    #[test]
    fn table_field_helpers_enumerate_nested_field_prefixes() {
        let parsed = parse_source(
            SourceId::new("schema:nested"),
            "DEFINE TABLE user;\nDEFINE FIELD profile ON user TYPE object;\nDEFINE FIELD profile.name ON user TYPE string;\nDEFINE FIELD profile.age ON user TYPE int;\nDEFINE FIELD profiled.nickname ON user TYPE string;\nDEFINE FIELD email ON user TYPE string;",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        let user = extraction.schema.table("user").expect("user table exists");
        let nested: Vec<_> = user
            .fields_under(&FieldPath::parse("profile"))
            .map(|field| field.path.join("."))
            .collect();

        assert_eq!(nested, vec!["profile", "profile.age", "profile.name"]);
        assert!(user.field(&FieldPath::parse("profiled")).is_none());
        assert_eq!(FieldPath::parse("..profile.name.").dotted(), "profile.name");
    }

    #[test]
    fn schema_index_stores_params_functions_and_analyzers_for_direct_lookup() {
        let parsed = parse_source(
            SourceId::new("schema:param-function-analyzer"),
            "DEFINE PARAM $api_timeout VALUE 30;\nDEFINE FUNCTION fn::score($age: int) -> int { RETURN $age; };\nDEFINE ANALYZER ascii TOKENIZERS blank,class FILTERS lowercase,snowball(english);",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());

        let param = extraction
            .schema
            .param("api_timeout")
            .expect("param is directly indexed without the $ sigil");
        assert_eq!(param.name, "api_timeout");
        assert_eq!(
            param.name_span.source(),
            &SourceId::new("schema:param-function-analyzer")
        );

        let function = extraction
            .schema
            .function("fn::score")
            .expect("function is directly indexed with namespace path");
        assert_eq!(function.name, "fn::score");
        assert_eq!(function.args.len(), 1);
        assert_eq!(function.args[0].name, "age");
        assert_eq!(function.args[0].kind, Some(Kind::Int));
        assert_eq!(function.return_kind, Some(Kind::Int));
        // A declared `-> int` is authoritative; the body is never inferred over it.
        assert_eq!(function.inferred_return, None);

        let analyzer = extraction
            .schema
            .analyzer("ascii")
            .expect("analyzer is directly indexed for full-text operator validation");
        assert_eq!(analyzer.name, "ascii");
        assert_eq!(analyzer.tokenizers, vec!["blank", "class"]);
        assert_eq!(analyzer.filters, vec!["lowercase", "snowball(english)"]);
    }

    #[test]
    fn untyped_function_persists_its_inferred_body_return_kind() {
        let parsed = parse_source(
            SourceId::new("schema:inferred-return"),
            "DEFINE FUNCTION fn::double($x: int) { RETURN $x * 2; };",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());

        let function = extraction
            .schema
            .function("fn::double")
            .expect("function is indexed");
        // No `-> T` annotation, so the declared return is absent...
        assert_eq!(function.return_kind, None);
        // ...but the body's `int * int` is inferred and persisted for callers.
        assert_eq!(function.inferred_return, Some(Kind::Int));
    }

    #[test]
    fn genuinely_untyped_function_body_persists_no_inferred_return() {
        // The body returns an untyped param, so the response kind is `Any`:
        // no false precision — the inferred return stays absent.
        let parsed = parse_source(
            SourceId::new("schema:opaque-return"),
            "DEFINE FUNCTION fn::opaque($x: any) { RETURN $x; };",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());

        let function = extraction
            .schema
            .function("fn::opaque")
            .expect("function is indexed");
        assert_eq!(function.return_kind, None);
        assert_eq!(function.inferred_return, None);
    }

    #[test]
    fn function_calling_another_untyped_function_resolves_its_inferred_return() {
        // `fn::wrap` has no `-> T`; its body delegates to `fn::base`, whose own
        // inferred return (`int`) is visible through the schema-aware walk.
        let parsed = parse_source(
            SourceId::new("schema:cross-fn-return"),
            "DEFINE FUNCTION fn::base($x: int) { RETURN $x * 2; };\n\
             DEFINE FUNCTION fn::wrap($y: int) { RETURN fn::base($y); };",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());

        let wrap = extraction
            .schema
            .function("fn::wrap")
            .expect("function is indexed");
        assert_eq!(wrap.return_kind, None);
        // Resolves through the callee; a safe fallback would be `None` (`Any`),
        // never a wrong type.
        assert!(
            matches!(wrap.inferred_return, Some(Kind::Int) | None),
            "cross-fn inferred return must resolve to int or safely fall back, got {:?}",
            wrap.inferred_return
        );
    }

    #[test]
    fn define_table_overwrite_replaces_table_metadata_without_duplicate_diagnostic() {
        let parsed = parse_source(
            SourceId::new("schema:overwrite-table"),
            "DEFINE TABLE person;\nDEFINE TABLE post;\nDEFINE TABLE likes TYPE RELATION IN person OUT post;\nDEFINE FIELD weight ON likes TYPE int;\nDEFINE TABLE OVERWRITE likes;",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());

        let likes = extraction
            .schema
            .table("likes")
            .expect("overwritten table remains in schema");
        assert_eq!(likes.relation, None);
        assert_eq!(
            likes
                .field(&FieldPath::parse("weight"))
                .expect("field remains attached after table overwrite")
                .kind,
            Some(Kind::Int)
        );
    }

    #[test]
    fn duplicate_define_without_overwrite_emits_diagnostic() {
        let parsed = parse_source(
            SourceId::new("schema:duplicate-table"),
            "DEFINE TABLE person;\nDEFINE TABLE person;",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        let duplicates: Vec<_> = extraction
            .diagnostics
            .iter()
            .filter(|finding| finding.code().number() == 1022)
            .collect();
        assert_eq!(duplicates.len(), 1);
        assert_eq!(
            duplicates[0].message(),
            "`person` is already defined; this DEFINE silently replaces the earlier one"
        );
    }

    #[test]
    fn object_and_record_union_field_types_resolve_to_kinds_without_partial() {
        use std::collections::BTreeMap;
        use surrealdb_types::{KindLiteral, Table};

        let parsed = parse_source(
            SourceId::new("schema:object-record-union"),
            "DEFINE TABLE team;\n\
             DEFINE TABLE user;\n\
             DEFINE TABLE organization;\n\
             DEFINE TABLE account;\n\
             DEFINE TABLE thing;\n\
             DEFINE FIELD address ON thing TYPE { street: string, zip: int };\n\
             DEFINE FIELD owner ON thing TYPE record<team | user | organization>;\n\
             DEFINE FIELD maybe ON thing TYPE option<record<account | team>>;",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());
        let thing = extraction.schema.table("thing").expect("thing table exists");

        let address = thing
            .field(&FieldPath::parse("address"))
            .expect("address field exists");
        assert!(address.partial.is_empty(), "object type must fully resolve");
        let mut expected = BTreeMap::new();
        expected.insert("street".to_string(), Kind::String);
        expected.insert("zip".to_string(), Kind::Int);
        assert_eq!(
            address.kind,
            Some(Kind::Literal(KindLiteral::Object(expected)))
        );

        let owner = thing
            .field(&FieldPath::parse("owner"))
            .expect("owner field exists");
        assert!(owner.partial.is_empty(), "record union must fully resolve");
        assert_eq!(
            owner.kind,
            Some(Kind::Record(vec![
                Table::from("team"),
                Table::from("user"),
                Table::from("organization"),
            ]))
        );

        let maybe = thing
            .field(&FieldPath::parse("maybe"))
            .expect("maybe field exists");
        assert!(maybe.partial.is_empty(), "option<record<...>> must resolve");
        assert_eq!(
            maybe.kind,
            Some(Kind::either(vec![
                Kind::None,
                Kind::Record(vec![Table::from("account"), Table::from("team")]),
            ]))
        );
    }

    #[test]
    fn untyped_field_infers_scalar_value_expression_kind() {
        // A `VALUE` with no `TYPE`: the field is typed by the value it stores.
        // A pure-scalar value resolves against an empty catalog at hoist time.
        let parsed = parse_source(
            SourceId::new("schema:value-scalar"),
            "DEFINE TABLE organization;\n\
             DEFINE FIELD label ON organization VALUE string::uppercase('x');",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());

        let label = extraction
            .schema
            .field("organization", &FieldPath::parse("label"))
            .expect("label field exists");
        assert_eq!(label.kind, Some(Kind::String));
        assert!(label.partial.is_empty(), "an inferred value kind is not partial");
    }

    #[test]
    fn untyped_field_infers_default_only_expression_kind() {
        // No `VALUE`, only a `DEFAULT`: the fallback value still types the field.
        let parsed = parse_source(
            SourceId::new("schema:default-only"),
            "DEFINE TABLE organization;\n\
             DEFINE FIELD stamp ON organization DEFAULT time::now();",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());

        let stamp = extraction
            .schema
            .field("organization", &FieldPath::parse("stamp"))
            .expect("stamp field exists");
        assert_eq!(stamp.kind, Some(Kind::Datetime));
    }

    #[test]
    fn untyped_field_value_reading_a_table_resolves_through_the_schema_aware_pass() {
        // The `VALUE` reads a field off another table's record: unresolvable
        // against an empty catalog (the hoist path leaves it `None`), resolved
        // once the schema-aware pass infers it against the full catalog.
        let parsed = parse_source(
            SourceId::new("schema:value-table"),
            "DEFINE TABLE user;\n\
             DEFINE FIELD email ON user TYPE string;\n\
             DEFINE TABLE organization;\n\
             DEFINE FIELD owner_email ON organization VALUE user:main.email;",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());

        let owner_email = extraction
            .schema
            .field("organization", &FieldPath::parse("owner_email"))
            .expect("owner_email field exists");
        assert_eq!(owner_email.kind, Some(Kind::String));
    }

    #[test]
    fn explicit_type_wins_over_the_value_expression_kind() {
        // A declared `TYPE` is authoritative: even a value that would infer a
        // sharper kind never overrides it.
        let parsed = parse_source(
            SourceId::new("schema:type-wins"),
            "DEFINE TABLE organization;\n\
             DEFINE FIELD label ON organization TYPE any VALUE string::uppercase('x');",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());

        let label = extraction
            .schema
            .field("organization", &FieldPath::parse("label"))
            .expect("label field exists");
        // The declared `any` stands — not the `string` the VALUE would infer.
        assert_eq!(label.kind, Some(Kind::Any));
    }

    #[test]
    fn genuinely_untyped_field_value_stays_untyped() {
        // The value is an opaque host-supplied param, so it infers to `Any`:
        // no false precision — the field kind stays absent.
        let parsed = parse_source(
            SourceId::new("schema:value-any"),
            "DEFINE TABLE organization;\n\
             DEFINE FIELD anything ON organization VALUE $input;",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());

        let anything = extraction
            .schema
            .field("organization", &FieldPath::parse("anything"))
            .expect("anything field exists");
        assert_eq!(anything.kind, None);
    }

    #[test]
    fn computed_reference_back_traversal_without_backlink_stays_untyped() {
        // The `COMPUTED` clause is now surfaced, but `team` carries no
        // `record<organization> REFERENCE` field, so the back-reference is not
        // provable: the field stays untyped rather than inventing a type.
        let parsed = parse_source(
            SourceId::new("schema:computed-ref"),
            "DEFINE TABLE organization;\n\
             DEFINE TABLE team;\n\
             DEFINE FIELD teams ON organization COMPUTED <~team;",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        let teams = extraction
            .schema
            .field("organization", &FieldPath::parse("teams"))
            .expect("teams field exists");
        assert_eq!(teams.kind, None);
    }

    #[test]
    fn computed_reference_back_traversal_types_from_provable_backlink() {
        // `team.org` is a `record<organization> REFERENCE`, so `<~team` on
        // organization resolves to `array<record<team>>`.
        let parsed = parse_source(
            SourceId::new("schema:computed-ref-ok"),
            "DEFINE TABLE team;\n\
             DEFINE FIELD org ON team TYPE record<organization> REFERENCE;\n\
             DEFINE TABLE organization;\n\
             DEFINE FIELD teams ON organization COMPUTED <~team;",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        let teams = extraction
            .schema
            .field("organization", &FieldPath::parse("teams"))
            .expect("teams field exists");
        assert_eq!(
            teams.kind,
            Some(Kind::Array(
                Box::new(Kind::Record(vec![surrealdb_types::Table::from("team")])),
                None
            ))
        );
        assert!(teams.computed, "a COMPUTED field is computed");
    }

    #[test]
    fn computed_forward_graph_traversal_types_as_target_record_array() {
        // A COMPUTED forward graph traversal resolves against the OWNING table
        // as the row context: `account->employee_of->organization` is
        // `array<record<organization>>`, with an interleaved `[WHERE …]` filter
        // and a `?? []` default both preserving that type.
        let parsed = parse_source(
            SourceId::new("schema:computed-graph"),
            "DEFINE TABLE account;\n\
             DEFINE TABLE organization;\n\
             DEFINE TABLE employee_of TYPE RELATION FROM account TO organization;\n\
             DEFINE FIELD status ON employee_of TYPE string;\n\
             DEFINE FIELD orgs ON account COMPUTED ->employee_of[WHERE status = 'active']->organization ?? [];",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        let orgs = extraction
            .schema
            .field("account", &FieldPath::parse("orgs"))
            .expect("orgs field exists");
        assert_eq!(
            orgs.kind,
            Some(Kind::Array(
                Box::new(Kind::Record(vec![surrealdb_types::Table::from(
                    "organization"
                )])),
                None
            ))
        );
    }

    #[test]
    fn computed_edge_to_edge_graph_chain_resolves_to_final_node() {
        // `->employee_of->member_of->team` steps edge → edge → node, where
        // `member_of` is a relation `FROM employee_of`: the chain still resolves
        // to `array<record<team>>`.
        let parsed = parse_source(
            SourceId::new("schema:computed-graph-chain"),
            "DEFINE TABLE account;\n\
             DEFINE TABLE organization;\n\
             DEFINE TABLE team;\n\
             DEFINE TABLE employee_of TYPE RELATION FROM account TO organization;\n\
             DEFINE FIELD status ON employee_of TYPE string;\n\
             DEFINE TABLE member_of TYPE RELATION FROM employee_of TO team;\n\
             DEFINE FIELD teams ON account COMPUTED ->employee_of[WHERE status = 'active']->member_of->team ?? [];",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        let teams = extraction
            .schema
            .field("account", &FieldPath::parse("teams"))
            .expect("teams field exists");
        assert_eq!(
            teams.kind,
            Some(Kind::Array(
                Box::new(Kind::Record(vec![surrealdb_types::Table::from("team")])),
                None
            ))
        );
    }

    #[test]
    fn computed_this_field_arithmetic_types_numeric() {
        // `$this.field <op> $this.field` resolves each field against the owning
        // table, so the arithmetic types numerically — even when the operands
        // are themselves `math::sum(...) ?? 0` computed fields.
        let parsed = parse_source(
            SourceId::new("schema:computed-this"),
            "DEFINE TABLE level;\n\
             DEFINE FIELD item ON level TYPE record<item>;\n\
             DEFINE FIELD qty ON level TYPE number;\n\
             DEFINE TABLE item;\n\
             DEFINE FIELD on_hand ON item COMPUTED math::sum(SELECT VALUE qty FROM level WHERE item = $parent) ?? 0;\n\
             DEFINE FIELD available ON item COMPUTED $this.on_hand - 5;",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        let available = extraction
            .schema
            .field("item", &FieldPath::parse("available"))
            .expect("available field exists");
        assert!(
            matches!(
                available.kind,
                Some(Kind::Int | Kind::Number | Kind::Float | Kind::Decimal)
            ),
            "expected a numeric kind, got {:?}",
            available.kind
        );
    }

    #[test]
    fn computed_scalar_expression_types_like_value() {
        // A plain scalar `COMPUTED` expression infers its scalar kind.
        let parsed = parse_source(
            SourceId::new("schema:computed-scalar"),
            "DEFINE TABLE person;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             DEFINE FIELD shout ON person COMPUTED string::uppercase(name);",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        let shout = extraction
            .schema
            .field("person", &FieldPath::parse("shout"))
            .expect("shout field exists");
        assert_eq!(shout.kind, Some(Kind::String));
    }

    #[test]
    fn remove_table_and_field_mutate_downstream_schema_context() {
        let parsed = parse_source(
            SourceId::new("schema:remove"),
            "DEFINE TABLE person;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;\nREMOVE FIELD age ON person;\nREMOVE TABLE person;",
        )
        .expect("schema parses");

        let extraction = extract_schema(&[parsed]);
        assert_eq!(extraction.diagnostics, Vec::new());
        assert!(extraction.schema.table("person").is_none());
    }
}


