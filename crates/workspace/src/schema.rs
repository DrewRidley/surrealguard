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
    /// The declared or inferred return kind, when known.
    pub return_kind: Option<Kind>,
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

/// A `DEFINE FIELD` definition: its declared kind and write semantics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDef {
    /// The field has a `DEFAULT` clause (or `VALUE`, which supplies one).
    pub has_default: bool,
    /// `READONLY` — writable only at creation.
    pub readonly: bool,
    /// `VALUE <expr>` — computed on write; hand-written values are
    /// overwritten.
    pub computed: bool,
    /// The field's dotted path split into segments.
    pub path: Vec<String>,
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
                findings.push(surrealguard_diagnostics::catalog::finding(
                    analyzer.name_span.clone(),
                    1032,
                    format!("`{tokenizer}` is not a tokenizer"),
                ));
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
                        findings.push(surrealguard_diagnostics::catalog::finding(
                            analyzer.name_span.clone(),
                            1032,
                            format!(
                                "`{}` is not a snowball language",
                                args.first().cloned().unwrap_or_default()
                            ),
                        ));
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
                _ => findings.push(surrealguard_diagnostics::catalog::finding(
                    analyzer.name_span.clone(),
                    1032,
                    format!("`{name}` is not a filter"),
                )),
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
    pub fn insert_field(&mut self, field: FieldDef, overwrite: bool) {
        let field_key = field.path.join(".");
        let Some(table) = self.tables.get_mut(&field.table) else {
            return;
        };
        if table.fields.contains_key(&field_key) && !overwrite {
            return;
        }
        table.fields.insert(field_key, field);
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
                schema.insert_field(field_def_from_ast(def, source, text), def.overwrite);
            }
            ast::DefineStmt::Index(def) => {
                let index = index_def_from_ast(def, source);
                if let Some(table) = schema.tables.get_mut(&index.table) {
                    table.indexes.insert(index.name.clone(), index);
                }
            }
            ast::DefineStmt::Param(def) => schema.insert_param(param_def_from_ast(def, source)),
            ast::DefineStmt::Function(def) => {
                schema.insert_function(function_def_from_ast(def, source, text, stmt.span));
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

/// Whether an index/event field path resolves on a table, either directly or
/// as the prefix of a declared nested field.
pub(crate) fn index_field_path_exists_on_table(table: &TableDef, path: &[String]) -> bool {
    let key = path.join(".");
    table.fields.contains_key(&key)
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
        None => (None, vec![PartialReason::Unresolved], None),
    };
    FieldDef {
        has_default: def.default.is_some() || def.value.is_some(),
        readonly: def.readonly,
        computed: def.value.is_some(),
        path: idiom_field_path(&def.path.node),
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
        source: source.clone(),
        name_span: span(source, def.name.span),
        return_span,
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
    use surrealdb_types::Kind;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    use super::{extract_schema, FieldPath};

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

        let analyzer = extraction
            .schema
            .analyzer("ascii")
            .expect("analyzer is directly indexed for full-text operator validation");
        assert_eq!(analyzer.name, "ascii");
        assert_eq!(analyzer.tokenizers, vec!["blank", "class"]);
        assert_eq!(analyzer.filters, vec!["lowercase", "snowball(english)"]);
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
            "duplicate table definition `person`"
        );
    }

    #[test]
    fn object_and_record_union_field_types_resolve_to_kinds_without_partial() {
        use std::collections::BTreeMap;
        use surrealdb_types::{KindLiteral, Table};

        let parsed = parse_source(
            SourceId::new("schema:object-record-union"),
            "DEFINE TABLE thing;\n\
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
