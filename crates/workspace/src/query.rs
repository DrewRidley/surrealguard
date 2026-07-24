//! Read-only queries over analysis output for editor features (inlay
//! hints, hover). This module produces no diagnostics and mutates no
//! state: it only reads the facts the analysis pipeline already computed
//! (`AnalysisOutput`, `SchemaIndex`) and shapes them for presentation.

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_syntax::ast;
use surrealguard_syntax::parse::parse_source;
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use crate::analysis::AnalysisOutput;
use crate::schema::SchemaIndex;

/// A type hint for a `LET $x = <expr>` binding: where the bound `$x` token
/// sits and the `: <kind>` label to render right after it (rust-analyzer
/// style grey inferred type).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeHint {
    /// The span of the `$name` token the hint annotates.
    pub name_span: SourceSpan,
    /// The rendered label, rendered as `<T>` (SurrealQL cast form).
    pub label: String,
}

/// The character budget past which an inlay label's rendered kind is elided
/// with `…`. Keeps a long object/union kind from dominating the line while
/// still signalling its shape.
const INLAY_LABEL_MAX: usize = 48;

/// Inferred-type inlay hints for every `LET` binding (and `FOR` loop
/// variable) whose kind analysis could determine — at every nesting depth
/// (top-level, `{ }` blocks, DEFINE FUNCTION bodies, FOR-loop bodies).
///
/// The label form is `: <kind>` placed right after the `$name` token
/// (rust-analyzer's grey inferred-type style), which the LSP anchors at the
/// name's end. Low-noise: bindings whose kind is undeterminable or `any`
/// produce no hint, and an over-long rendered kind is elided with `…`.
/// Duplicate bindings at the same span (should not occur, but re-inference
/// could) are emitted once.
pub fn let_binding_hints(output: &AnalysisOutput) -> Vec<TypeHint> {
    let mut seen: std::collections::HashSet<(SourceId, u32, u32)> = std::collections::HashSet::new();
    output
        .let_bindings
        .iter()
        .filter_map(|binding| {
            let kind = binding.kind.as_ref()?;
            // Low-FP / low-noise: `any` (and its `none` degenerate) carry no
            // information worth an inline annotation.
            if matches!(kind, Kind::Any) {
                return None;
            }
            let range = binding.name_span.range();
            if !seen.insert((
                binding.name_span.source().clone(),
                range.start(),
                range.end(),
            )) {
                return None;
            }
            Some(TypeHint {
                // SurrealQL's own cast syntax `<T>` reads more naturally than a
                // Rust-style `: T` for an inline type ghost.
                name_span: binding.name_span.clone(),
                label: format!("<{}>", elide_label(&render_kind(kind))),
            })
        })
        .collect()
}

/// Truncates a rendered kind to [`INLAY_LABEL_MAX`] characters, appending `…`
/// when it overruns. Operates on chars so a multibyte boundary is never split.
fn elide_label(rendered: &str) -> String {
    if rendered.chars().count() <= INLAY_LABEL_MAX {
        return rendered.to_string();
    }
    let mut out: String = rendered.chars().take(INLAY_LABEL_MAX).collect();
    out.push('…');
    out
}

/// A resolved hover: the covered symbol's span and a markdown popover
/// describing its inferred type (and, for records/objects, its fields).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoverInfo {
    /// The span of the symbol the hover describes.
    pub span: SourceSpan,
    /// The markdown body to show.
    pub markdown: String,
}

/// Resolves a hover at byte `offset` in `source`: maps the cursor to the
/// smallest covering symbol among the `LET` bindings, parameter uses, table
/// declarations, and the context params (`$value`/`$event`/`$after`/...)
/// bound by an enclosing DEFINE construct, and renders its inferred type.
/// `None` when the cursor is over nothing typed. `text` is the source's full
/// text, needed to locate the enclosing DEFINE construct for context params.
pub fn hover_at(
    output: &AnalysisOutput,
    schema: &SchemaIndex,
    source: &SourceId,
    text: &str,
    offset: u32,
) -> Option<HoverInfo> {
    let mut best: Option<(u32, HoverInfo)> = None;
    let mut consider = |span: &SourceSpan, markdown: String| {
        if span.source() != source {
            return;
        }
        let range = span.range();
        if offset < range.start() || offset > range.end() {
            return;
        }
        let width = range.end().saturating_sub(range.start());
        let is_smaller = match &best {
            None => true,
            Some((best_width, _)) => width < *best_width,
        };
        if is_smaller {
            best = Some((
                width,
                HoverInfo {
                    span: span.clone(),
                    markdown,
                },
            ));
        }
    };

    // `LET $x` / `FOR $x` binding sites (every nesting depth).
    for binding in &output.let_bindings {
        consider(
            &binding.name_span,
            symbol_markdown(
                Some(&format!("local `${}`", binding.name)),
                &format!("${}", binding.name),
                binding.kind.as_ref(),
                schema,
            ),
        );
    }

    // The kind of each `LET`/`FOR` variable, by name, for resolving `$var`
    // uses and `$var[i].field` idioms anywhere in the source. A name bound
    // more than once keeps the last binding's kind (source-order shadowing).
    let mut let_kinds: std::collections::HashMap<String, Kind> =
        std::collections::HashMap::new();
    for binding in &output.let_bindings {
        if let Some(kind) = &binding.kind {
            let_kinds.insert(binding.name.clone(), kind.clone());
        }
    }

    // `$param` use sites.
    for param in &output.inferred_params {
        let markdown = symbol_markdown(
            Some(&format!("parameter `${}`", param.name)),
            &format!("${}", param.name),
            param.kind.as_ref(),
            schema,
        );
        for span in &param.spans {
            consider(span, markdown.clone());
        }
    }

    // `DEFINE TABLE` declaration names.
    for table in schema.tables.values() {
        consider(&table.name_span, table_markdown(table, schema));
    }

    // Context params (`$value`/`$event`/`$before`/...) bound by the DEFINE
    // construct enclosing the cursor. Located lexically — the token under the
    // cursor is looked up in the construct's binding map.
    if let Some((name, range)) = crate::context_params::param_token_at(text, offset) {
        if let Some(map) = crate::context_params::context_param_map(schema, source, text, offset) {
            if let Some(kind) = map.get(&name) {
                let span = SourceSpan::new(source.clone(), range);
                consider(
                    &span,
                    symbol_markdown(
                        Some(&format!("context `${name}`")),
                        &format!("${name}"),
                        Some(kind),
                        schema,
                    ),
                );
            }
        }
    }

    // Table references and field names in ordinary positions. The analyzed
    // statements carry no per-reference spans, so — as with context params —
    // a fresh parse is walked to locate the identifier under the cursor and
    // resolve it against the schema. Only definitely-typed references produce
    // a hover; anything uncertain is left untouched (low-FP).
    if let Ok(parsed) = parse_source(source.clone(), text) {
        let statements = surrealguard_syntax::lower::lower_statements(&parsed);
        let mut collector = SchemaHovers {
            schema,
            source,
            offset,
            let_kinds: &let_kinds,
            param_kinds: std::collections::HashMap::new(),
            out: Vec::new(),
        };
        for statement in &statements {
            collector.walk_statement(statement);
        }
        for (span, markdown) in collector.out {
            consider(&span, markdown);
        }
    }

    best.map(|(_, info)| info)
}

/// A resolved go-to-definition target: the span of the DEFINE construct a
/// reference under the cursor points at. The span carries its own source id,
/// so the definition may live in a different `.surql` file than the reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefinitionTarget {
    /// The span of the definition's name (the `DEFINE TABLE`/`DEFINE FIELD`/
    /// `DEFINE FUNCTION` name, or a `LET`/`DEFINE PARAM` binding site).
    pub span: SourceSpan,
}

/// Resolves a go-to-definition at byte `offset` in `source`: maps the cursor to
/// the smallest covering reference and returns the span of the definition it
/// points at. Mirrors [`hover_at`]'s position→symbol logic but yields a
/// location instead of markdown.
///
/// Resolves table references (FROM, `record<T>`, `DEFINE … ON`, targets, graph
/// endpoints) to their `DEFINE TABLE` name, field references (projections,
/// WHERE, SET, idioms, across `record<>` links) to the owning `DEFINE FIELD`
/// name, `fn::` calls to their `DEFINE FUNCTION`, and `$param`/`LET` uses to
/// their binding site. Returns `None` when the definition span isn't known —
/// a schemaless field, an unknown table, an implicit `id`/`in`/`out`, or an
/// opaque traversal — so a cmd-click never jumps to the wrong place.
pub fn definition_at(
    output: &AnalysisOutput,
    schema: &SchemaIndex,
    source: &SourceId,
    text: &str,
    offset: u32,
) -> Option<DefinitionTarget> {
    let mut best: Option<(u32, SourceSpan)> = None;
    let mut consider = |cover: ByteRange, target: SourceSpan| {
        if offset < cover.start() || offset > cover.end() {
            return;
        }
        let width = cover.end().saturating_sub(cover.start());
        let is_smaller = best.as_ref().map_or(true, |(best_width, _)| width < *best_width);
        if is_smaller {
            best = Some((width, target));
        }
    };

    // `$param` / `LET` variable binding sites, keyed by name: a `LET $name`
    // binding in this output takes precedence over a global `DEFINE PARAM`.
    // Resolved against the walked `$name` tokens below.
    let mut bindings: std::collections::HashMap<String, SourceSpan> =
        std::collections::HashMap::new();
    for binding in &output.let_bindings {
        bindings
            .entry(binding.name.clone())
            .or_insert_with(|| binding.name_span.clone());
    }
    for param in schema.params.values() {
        bindings
            .entry(param.name.clone())
            .or_insert_with(|| param.name_span.clone());
    }

    // Table, field, and `fn::` references resolved by walking a freshly-lowered
    // statement tree — the analyzed statements carry no per-reference spans, so
    // (as hover does) the identifier under the cursor is located here and
    // resolved against the schema. Only definitely-known definitions produce a
    // target; anything uncertain is skipped (low-FP).
    if let Ok(parsed) = parse_source(source.clone(), text) {
        let statements = surrealguard_syntax::lower::lower_statements(&parsed);
        let mut collector = SchemaDefs {
            schema,
            source,
            offset,
            bindings: &bindings,
            param_spans: std::collections::HashMap::new(),
            out: Vec::new(),
        };
        for statement in &statements {
            collector.walk_statement(statement);
        }
        for (cover, target) in collector.out {
            consider(cover, target);
        }
    }

    best.map(|(_, span)| DefinitionTarget { span })
}

/// Markdown for a variable/parameter/field symbol: an optional bold caption
/// line (already-formatted markdown, e.g. ``local `$direct` ``) above a fenced
/// `surql` type line the editor syntax-highlights. Records and literal objects
/// additionally get their field shape as an indented `surql` block.
fn symbol_markdown(
    caption: Option<&str>,
    name: &str,
    kind: Option<&Kind>,
    schema: &SchemaIndex,
) -> String {
    let rendered = kind.map_or_else(|| "unknown".to_string(), render_kind);
    let mut markdown = String::new();
    if let Some(caption) = caption {
        markdown.push_str(&format!("**{caption}**\n"));
    }
    markdown.push_str(&format!("```surql\n{name}: {rendered}\n```"));
    // Only records earn an expanded field block: their linked-table fields
    // aren't visible in the compact `record<t>` render, so listing them adds
    // information. Object/array kinds already render their shape inline, so a
    // second copy would just be noise.
    if let Some(kind) = kind {
        if is_record_bearing(kind) {
            let fields = field_lines(kind, schema);
            if !fields.is_empty() {
                markdown.push_str("\n```surql\n");
                for line in &fields {
                    markdown.push_str(line);
                    markdown.push('\n');
                }
                markdown.push_str("```");
            }
        }
    }
    markdown
}

/// The field-line cap for a table popover: enough to convey the shape without
/// the hover swallowing the screen on a wide table.
const TABLE_FIELD_CAP: usize = 20;

/// Markdown for a table declaration: a fenced `surql` preview built as a
/// `DEFINE TABLE` line (so editors color the keywords and the table name),
/// the `TYPE RELATION` edge spec when it is a relation, and one `field: kind`
/// line per declared field (capped, with a `-- +N more` tail when it overruns).
fn table_markdown(table: &crate::schema::TableDef, schema: &SchemaIndex) -> String {
    let mut body = format!("DEFINE TABLE {}", table.name);
    if table.schemafull {
        body.push_str(" SCHEMAFULL");
    }
    body.push(';');
    if let Some(relation) = &table.relation {
        body.push('\n');
        body.push_str(&relation_line(relation));
    }
    let fields = table_field_lines(&table.name, schema);
    for line in fields.iter().take(TABLE_FIELD_CAP) {
        body.push('\n');
        body.push_str(line);
    }
    if fields.len() > TABLE_FIELD_CAP {
        body.push_str(&format!("\n-- +{} more", fields.len() - TABLE_FIELD_CAP));
    }
    format!("```surql\n{body}\n```")
}

/// The `TYPE RELATION IN <a> OUT <b>` line for a relation edge's popover.
/// An empty endpoint list (any record) renders as a bare `IN`/`OUT`.
fn relation_line(relation: &crate::schema::RelationDef) -> String {
    let mut line = "TYPE RELATION".to_string();
    if !relation.in_tables.is_empty() {
        line.push_str(&format!(" IN {}", relation.in_tables.join(" | ")));
    }
    if !relation.out_tables.is_empty() {
        line.push_str(&format!(" OUT {}", relation.out_tables.join(" | ")));
    }
    line
}

/// Whether a kind carries record links whose fields are worth expanding in a
/// popover (a `record<>`, or an `option<record<>>`/union of them).
fn is_record_bearing(kind: &Kind) -> bool {
    match kind {
        Kind::Record(_) => true,
        Kind::Either(variants) => variants.iter().any(is_record_bearing),
        _ => false,
    }
}

/// `name: kind` lines for a kind that carries a field shape: the fields of
/// each record table it names, or the entries of a literal object.
fn field_lines(kind: &Kind, schema: &SchemaIndex) -> Vec<String> {
    match kind {
        Kind::Record(tables) => tables
            .iter()
            .flat_map(|table| table_field_lines(table.as_str(), schema))
            .collect(),
        Kind::Literal(KindLiteral::Object(entries)) => entries
            .iter()
            .map(|(name, kind)| format!("{name}: {}", render_kind(kind)))
            .collect(),
        // Unwrap `option<record<..>>` etc. to the wrapped record shape.
        Kind::Either(variants) => variants
            .iter()
            .filter(|variant| !matches!(variant, Kind::None | Kind::Null))
            .flat_map(|variant| field_lines(variant, schema))
            .collect(),
        _ => Vec::new(),
    }
}

/// `name: kind` lines for a declared table's fields.
fn table_field_lines(table: &str, schema: &SchemaIndex) -> Vec<String> {
    let Some(def) = schema.table(table) else {
        return Vec::new();
    };
    def.fields
        .values()
        .map(|field| {
            let kind = field.kind.as_ref().map_or_else(|| "any".to_string(), render_kind);
            format!("{}: {kind}", field.path.join("."))
        })
        .collect()
}

/// Renders a [`Kind`] compactly for editor surfaces: `record<file>`,
/// `array<{ name: string }>`, `option<string>`. Falls back to the kind's
/// own `Display` for shapes without a special compact form.
pub fn render_kind(kind: &Kind) -> String {
    match kind {
        Kind::Any => "any".to_string(),
        Kind::None => "none".to_string(),
        Kind::Null => "null".to_string(),
        Kind::Bool => "bool".to_string(),
        Kind::Bytes => "bytes".to_string(),
        Kind::Datetime => "datetime".to_string(),
        Kind::Decimal => "decimal".to_string(),
        Kind::Duration => "duration".to_string(),
        Kind::Float => "float".to_string(),
        Kind::Int => "int".to_string(),
        Kind::Number => "number".to_string(),
        Kind::Object => "object".to_string(),
        Kind::String => "string".to_string(),
        Kind::Uuid => "uuid".to_string(),
        Kind::Regex => "regex".to_string(),
        Kind::Range => "range".to_string(),
        Kind::Record(tables) => wrap_tables("record", tables),
        Kind::Table(tables) => wrap_tables("table", tables),
        Kind::Array(inner, len) => wrap_collection("array", inner, *len),
        Kind::Set(inner, len) => wrap_collection("set", inner, *len),
        Kind::File(buckets) => {
            if buckets.is_empty() {
                "file".to_string()
            } else {
                format!("file<{}>", buckets.join(", "))
            }
        }
        Kind::Either(variants) => render_either(variants),
        Kind::Literal(literal) => render_literal(literal),
        // Geometry, Function, and any future variant: defer to Display.
        other => other.to_string(),
    }
}

fn wrap_tables(head: &str, tables: &[surrealdb_types::Table]) -> String {
    if tables.is_empty() {
        head.to_string()
    } else {
        let names: Vec<&str> = tables.iter().map(surrealdb_types::Table::as_str).collect();
        format!("{head}<{}>", names.join(" | "))
    }
}

fn wrap_collection(head: &str, inner: &Kind, len: Option<u64>) -> String {
    match len {
        Some(len) => format!("{head}<{}, {len}>", render_kind(inner)),
        None => format!("{head}<{}>", render_kind(inner)),
    }
}

/// `option<T>` when the union is `none` plus other variants; otherwise a
/// `a | b | c` union.
fn render_either(variants: &[Kind]) -> String {
    let has_none = variants.iter().any(|kind| matches!(kind, Kind::None));
    let rest: Vec<String> = variants
        .iter()
        .filter(|kind| !matches!(kind, Kind::None))
        .map(render_kind)
        .collect();
    if rest.is_empty() {
        return "none".to_string();
    }
    let joined = rest.join(" | ");
    if has_none {
        format!("option<{joined}>")
    } else {
        joined
    }
}

fn render_literal(literal: &KindLiteral) -> String {
    match literal {
        KindLiteral::String(value) => format!("'{value}'"),
        KindLiteral::Integer(value) => value.to_string(),
        KindLiteral::Float(value) => value.to_string(),
        KindLiteral::Decimal(value) => value.to_string(),
        KindLiteral::Duration(value) => value.to_string(),
        KindLiteral::Bool(value) => value.to_string(),
        KindLiteral::Array(kinds) => {
            let rendered: Vec<String> = kinds.iter().map(render_kind).collect();
            format!("[{}]", rendered.join(", "))
        }
        KindLiteral::Object(entries) => {
            let rendered: Vec<String> = entries
                .iter()
                .map(|(name, kind)| format!("{name}: {}", render_kind(kind)))
                .collect();
            format!("{{ {} }}", rendered.join(", "))
        }
    }
}

/// Collects hover candidates for table references and field names by walking
/// a freshly-lowered statement tree. Each candidate is a `(span, markdown)`
/// pair the caller feeds through the smallest-covering `consider` closure.
///
/// Field resolution tracks the table in scope (a SELECT's single `FROM`
/// source, a mutation's target, a `DEFINE FIELD`'s table) and follows
/// `record<>` links: `author.name` on `post` resolves `name` against the
/// linked `user`. Ambiguity (multiple sources, an opaque traversal) drops the
/// scope so nothing misleading is shown.
struct SchemaHovers<'a> {
    schema: &'a SchemaIndex,
    source: &'a SourceId,
    offset: u32,
    /// The inferred kind of each `LET`/`FOR` variable in scope, by name,
    /// for hovering `$var` uses and `$var[i].field` idioms.
    let_kinds: &'a std::collections::HashMap<String, Kind>,
    /// Declared `DEFINE FUNCTION` parameters in scope while walking a function
    /// body, by name → declared kind. Populated on entry to the body and
    /// restored on exit, so `$param` uses deep in the body hover their declared
    /// type. A `LET` of the same name shadows it (`let_kinds` is checked first).
    param_kinds: std::collections::HashMap<String, Kind>,
    out: Vec<(SourceSpan, String)>,
}

impl SchemaHovers<'_> {
    fn covers(&self, span: ByteRange) -> bool {
        self.offset >= span.start() && self.offset <= span.end()
    }

    /// The kind of a `$var` in scope: a `LET`/`FOR` binding wins over a
    /// function parameter of the same name.
    fn var_kind(&self, name: &str) -> Option<Kind> {
        self.let_kinds
            .get(name)
            .or_else(|| self.param_kinds.get(name))
            .cloned()
    }

    /// The bold caption for a `$var` hover: `local` for a `LET`/`FOR`
    /// binding, `parameter` for a function parameter.
    fn var_caption(&self, name: &str) -> String {
        if self.let_kinds.contains_key(name) {
            format!("local `${name}`")
        } else {
            format!("parameter `${name}`")
        }
    }

    /// A table reference (`FROM person`, `ON person`, a graph edge): show the
    /// `table <name>` popover, but only for a table the schema actually knows.
    fn table_ref(&mut self, name: &ast::Spanned<String>) {
        if !self.covers(name.span) {
            return;
        }
        if let Some(def) = self.schema.table(&name.node) {
            let span = SourceSpan::new(self.source.clone(), name.span);
            self.out.push((span, table_markdown(def, self.schema)));
        }
    }

    /// Walks a statement, threading the row table into the positions where
    /// field paths resolve against it.
    fn walk_statement(&mut self, statement: &ast::Spanned<ast::Statement>) {
        use ast::Statement;
        match &statement.node {
            Statement::Select(select) => {
                let root = single_table(&select.from);
                for source in &select.from {
                    self.walk_expr(None, source);
                }
                for projection in &select.projections {
                    if let ast::Projection::Expr { expr, .. } = projection {
                        self.walk_expr(root, expr);
                    }
                }
                if let Some(where_clause) = &select.where_clause {
                    self.walk_expr(root, where_clause);
                }
                if let Some(group) = &select.group {
                    for key in &group.keys {
                        self.walk_idiom(root, &key.node);
                    }
                }
                if let Some(order) = &select.order {
                    for key in &order.keys {
                        self.walk_expr(root, &key.expr);
                    }
                }
                for idiom in select.omit.iter().chain(&select.split).chain(&select.fetch) {
                    self.walk_idiom(root, &idiom.node);
                }
                for extra in [&select.limit, &select.start, &select.timeout].into_iter().flatten() {
                    self.walk_expr(None, extra);
                }
            }
            Statement::Create(create) => {
                let root = single_table(&create.targets);
                for target in &create.targets {
                    self.walk_expr(None, target);
                }
                self.walk_data(root, create.data.as_ref());
            }
            Statement::Update(update) => {
                let root = single_table(&update.targets);
                for target in &update.targets {
                    self.walk_expr(None, target);
                }
                self.walk_data(root, update.data.as_ref());
                if let Some(where_clause) = &update.where_clause {
                    self.walk_expr(root, where_clause);
                }
            }
            Statement::Upsert(upsert) => {
                let root = single_table(&upsert.targets);
                for target in &upsert.targets {
                    self.walk_expr(None, target);
                }
                self.walk_data(root, upsert.data.as_ref());
                if let Some(where_clause) = &upsert.where_clause {
                    self.walk_expr(root, where_clause);
                }
            }
            Statement::Delete(delete) => {
                let root = single_table(&delete.targets);
                for target in &delete.targets {
                    self.walk_expr(None, target);
                }
                if let Some(where_clause) = &delete.where_clause {
                    self.walk_expr(root, where_clause);
                }
            }
            Statement::Insert(insert) => {
                let root = insert
                    .target
                    .as_ref()
                    .and_then(|target| expr_table_name(&target.node));
                if let Some(target) = &insert.target {
                    self.walk_expr(None, target);
                }
                self.walk_insert_data(root, &insert.data);
            }
            Statement::Relate(relate) => {
                for endpoint in [&relate.from, &relate.edge, &relate.to].into_iter().flatten() {
                    self.walk_expr(None, endpoint);
                }
                let root = relate.edge.as_ref().and_then(|edge| expr_table_name(&edge.node));
                self.walk_data(root, relate.data.as_ref());
            }
            Statement::Define(define) => self.walk_define(define),
            Statement::Remove(remove) => match &remove.target {
                ast::RemoveTarget::Table(name) => self.table_ref(name),
                ast::RemoveTarget::Field { field, table } => {
                    self.table_ref(table);
                    self.walk_idiom(Some(table.node.as_str()), &field.node);
                }
                ast::RemoveTarget::Index { table, .. } => self.table_ref(table),
                ast::RemoveTarget::Other(_) => {}
            },
            Statement::Alter(alter) => {
                if let Some(table) = &alter.table {
                    self.table_ref(table);
                }
            }
            Statement::LiveSelect(live) => {
                if let Some(table) = &live.table {
                    self.table_ref(table);
                }
            }
            Statement::Info(info) => {
                if let Some(table) = &info.table {
                    self.table_ref(table);
                }
            }
            Statement::Show(show) => {
                if let Some(table) = &show.table {
                    self.table_ref(table);
                }
            }
            Statement::Rebuild(rebuild) => {
                if let Some(table) = &rebuild.table {
                    self.table_ref(table);
                }
            }
            Statement::Let(let_stmt) => self.walk_expr(None, &let_stmt.value),
            Statement::Return(ret) => {
                if let Some(value) = &ret.value {
                    self.walk_expr(None, value);
                }
            }
            Statement::Throw(throw) => {
                if let Some(value) = &throw.value {
                    self.walk_expr(None, value);
                }
            }
            Statement::Kill(kill) => {
                if let Some(id) = &kill.id {
                    self.walk_expr(None, id);
                }
            }
            Statement::IfElse(if_else) => {
                for branch in &if_else.branches {
                    self.walk_expr(None, &branch.condition);
                    self.walk_block(&branch.body);
                }
                if let Some(else_branch) = &if_else.else_branch {
                    self.walk_block(else_branch);
                }
            }
            Statement::For(for_stmt) => {
                self.walk_expr(None, &for_stmt.iterable);
                self.walk_block(&for_stmt.body);
            }
            Statement::Block(block) => self.walk_block(block),
            Statement::Expr(expr) => self.walk_expr(None, expr),
            _ => {}
        }
    }

    fn walk_define(&mut self, define: &ast::DefineStmt) {
        use ast::DefineStmt;
        match define {
            DefineStmt::Field(field) => {
                self.table_ref(&field.table);
                self.walk_idiom(Some(field.table.node.as_str()), &field.path.node);
                let root = Some(field.table.node.as_str());
                for expr in [&field.default, &field.value, &field.assert].into_iter().flatten() {
                    self.walk_expr(root, expr);
                }
                for predicate in &field.permissions {
                    self.walk_expr(root, predicate);
                }
            }
            DefineStmt::Table(table) => {
                if let Some(relation) = &table.relation {
                    for endpoint in relation.in_tables.iter().chain(&relation.out_tables) {
                        self.table_ref(endpoint);
                    }
                }
                let root = Some(table.name.node.as_str());
                for predicate in &table.permissions {
                    self.walk_expr(root, predicate);
                }
            }
            DefineStmt::Index(index) => {
                self.table_ref(&index.table);
                for idiom in &index.fields {
                    self.walk_idiom(Some(index.table.node.as_str()), &idiom.node);
                }
            }
            DefineStmt::Event(event) => {
                self.table_ref(&event.table);
                let root = Some(event.table.node.as_str());
                for expr in [&event.when, &event.then].into_iter().flatten() {
                    self.walk_expr(root, expr);
                }
            }
            // A DEFINE FUNCTION body is ordinary statement territory: its
            // `LET`/`FOR` var uses, `fn::` calls, and idioms all resolve the
            // same as at top level, so walk it — but first bring the declared
            // parameters into scope so `$param` uses in the body (and the
            // signature tokens themselves) hover their declared type.
            DefineStmt::Function(func) => {
                let def = self.schema.function(&func.name.node);
                let saved = std::mem::take(&mut self.param_kinds);
                for (name, _ty) in &func.params {
                    let kind = def
                        .and_then(|def| def.args.iter().find(|arg| arg.name == name.node))
                        .and_then(|arg| arg.kind.clone());
                    if let Some(kind) = kind {
                        // Hover the `$param` token in the signature itself.
                        if self.covers(name.span) {
                            let span = SourceSpan::new(self.source.clone(), name.span);
                            self.out.push((
                                span,
                                symbol_markdown(
                                    Some(&format!("parameter `${}`", name.node)),
                                    &format!("${}", name.node),
                                    Some(&kind),
                                    self.schema,
                                ),
                            ));
                        }
                        self.param_kinds.insert(name.node.clone(), kind);
                    }
                }
                if let Some(body) = &func.body {
                    self.walk_block(body);
                }
                self.param_kinds = saved;
            }
            _ => {}
        }
    }

    fn walk_block(&mut self, block: &ast::Block) {
        for statement in &block.statements {
            self.walk_statement(statement);
        }
    }

    fn walk_data(&mut self, root: Option<&str>, data: Option<&ast::DataClause>) {
        use ast::DataClause;
        match data {
            Some(DataClause::Set(assignments)) => {
                for assignment in assignments {
                    self.walk_idiom(root, &assignment.target.node);
                    self.walk_expr(root, &assignment.value);
                }
            }
            Some(DataClause::Unset(idioms)) => {
                for idiom in idioms {
                    self.walk_idiom(root, &idiom.node);
                }
            }
            Some(
                DataClause::Content(expr)
                | DataClause::Merge(expr)
                | DataClause::Patch(expr)
                | DataClause::Replace(expr)
                | DataClause::Single(expr),
            ) => self.walk_expr(root, expr),
            _ => {}
        }
    }

    fn walk_insert_data(&mut self, root: Option<&str>, data: &ast::InsertData) {
        use ast::InsertData;
        match data {
            InsertData::Values(exprs) => {
                for expr in exprs {
                    self.walk_expr(root, expr);
                }
            }
            InsertData::Rows { rows, .. } => {
                for row in rows {
                    for (column, value) in row {
                        self.walk_idiom(root, &column.node);
                        self.walk_expr(root, value);
                    }
                }
            }
            InsertData::Assignments(assignments) => {
                for (column, value) in assignments {
                    self.walk_idiom(root, &column.node);
                    self.walk_expr(root, value);
                }
            }
            InsertData::Partial(_) => {}
        }
    }

    /// Recurses through an expression, resolving table references and
    /// field-path idioms against the row table `root` (when one is in scope).
    fn walk_expr(&mut self, root: Option<&str>, expr: &ast::Spanned<ast::Expr>) {
        use ast::Expr;
        match &expr.node {
            Expr::Table(name) => self.table_ref(name),
            Expr::RecordId { table, .. } => self.table_ref(table),
            Expr::Param(name) => self.param_use(expr.span, name),
            Expr::Idiom(idiom) => self.walk_idiom(root, idiom),
            Expr::Binary { lhs, rhs, .. } => {
                self.walk_expr(root, lhs);
                self.walk_expr(root, rhs);
            }
            Expr::Prefix { expr, .. } | Expr::Cast { expr, .. } => self.walk_expr(root, expr),
            Expr::Call(call) => {
                self.call_signature(call);
                for arg in &call.args {
                    self.walk_expr(root, arg);
                }
            }
            Expr::Object(entries) => {
                for (_, value) in entries {
                    self.walk_expr(root, value);
                }
            }
            Expr::Array(items) => {
                for item in items {
                    self.walk_expr(root, item);
                }
            }
            Expr::Subquery(statement) => self.walk_statement(statement),
            Expr::Block(block) => self.walk_block(block),
            Expr::Closure(closure) => self.walk_expr(root, &closure.body),
            _ => {}
        }
    }

    /// A bare `$var` use whose `LET`/`FOR`/function-parameter kind is known →
    /// its inferred type.
    fn param_use(&mut self, span: ByteRange, name: &str) {
        if !self.covers(span) {
            return;
        }
        if let Some(kind) = self.var_kind(name) {
            let source_span = SourceSpan::new(self.source.clone(), span);
            self.out.push((
                source_span,
                symbol_markdown(Some(&self.var_caption(name)), &format!("${name}"), Some(&kind), self.schema),
            ));
        }
    }

    /// A `fn::` call whose path the cursor is over → a signature popover
    /// (`fn::name($p: kind, ...) -> return`) built from the `DEFINE FUNCTION`.
    fn call_signature(&mut self, call: &ast::Call) {
        if !self.covers(call.path.span) {
            return;
        }
        if let Some(func) = self.schema.function(&call.path.node) {
            let span = SourceSpan::new(self.source.clone(), call.path.span);
            self.out.push((span, function_signature_markdown(func)));
        }
    }

    /// Resolves an idiom rooted in a known-kind `$var` (`$direct[0].role`):
    /// steps a value kind through subscripts (into the array/set element) and
    /// fields (into a literal object's entry, or across a single `record<>`
    /// link into schema-resolved fields), emitting a hover for the `$var`
    /// token and each resolvable segment. Returns whether it consumed the
    /// idiom (so the table-based walker can skip it).
    fn resolve_value_idiom(&mut self, name: &str, name_span: ByteRange, idiom: &ast::Idiom) {
        use ast::IdiomPart;
        // Hover the leading `$var` token itself.
        let Some(root_kind) = self.var_kind(name) else {
            return;
        };
        if self.covers(name_span) {
            let span = SourceSpan::new(self.source.clone(), name_span);
            self.out.push((
                span,
                symbol_markdown(Some(&self.var_caption(name)), &format!("${name}"), Some(&root_kind), self.schema),
            ));
        }
        // `current` is a value kind; once traversal crosses a `record<>` link
        // it switches to `table` mode and the schema-based resolver takes
        // over (the same code path the row-table walker uses).
        let mut current = Some(root_kind);
        let mut table: Option<String> = None;
        let mut segments: Vec<String> = Vec::new();
        for part in idiom.parts.iter().skip(1) {
            match &part.node {
                IdiomPart::Index(inner) => {
                    self.walk_expr(None, inner);
                    current = current.as_ref().and_then(element_kind);
                    table = None;
                    segments.clear();
                }
                IdiomPart::All | IdiomPart::Last => {
                    current = current.as_ref().and_then(element_kind);
                    table = None;
                    segments.clear();
                }
                IdiomPart::Field(field) => {
                    // A value kind that is a single `record<>` link enters its
                    // schema table before this field resolves.
                    if table.is_none() {
                        if let Some(linked) = current.as_ref().and_then(record_link_target) {
                            table = Some(linked);
                            segments.clear();
                            current = None;
                        }
                    }
                    // Schema-table mode: resolve the field against the table.
                    if let Some(current_table) = table.clone() {
                        self.resolve_table_field(
                            &current_table,
                            &mut segments,
                            field,
                            part.span,
                            &mut table,
                        );
                        continue;
                    }
                    // Object-value mode: index into a literal object's entry.
                    let Some(kind) = current.clone() else {
                        return;
                    };
                    let next = field_kind(&kind, field);
                    if self.covers(part.span) {
                        if let Some(next) = &next {
                            let span = SourceSpan::new(self.source.clone(), part.span);
                            self.out.push((
                                span,
                                symbol_markdown(
                                    Some(&format!("field `{field}`")),
                                    field,
                                    Some(next),
                                    self.schema,
                                ),
                            ));
                        }
                    }
                    current = next;
                }
                // Anything else (methods, graph steps, where) is opaque here.
                _ => return,
            }
        }
    }

    /// Resolves `field` under `table`/`segments` (a schema table reached
    /// across a record link), emitting its hover and advancing scope
    /// (`out_table` re-roots on a further link; `segments` grows on a nested
    /// object; both clear when the shape goes opaque).
    fn resolve_table_field(
        &mut self,
        table: &str,
        segments: &mut Vec<String>,
        field: &str,
        span: ByteRange,
        out_table: &mut Option<String>,
    ) {
        let Some(def) = self.schema.table(table) else {
            *out_table = None;
            return;
        };
        let mut path = segments.clone();
        path.push(field.to_string());
        let kind = crate::analyzer::data::select::kind_for_path(def, &path);
        if self.covers(span) {
            if let Some(kind) = &kind {
                let source_span = SourceSpan::new(self.source.clone(), span);
                self.out.push((
                    source_span,
                    symbol_markdown(
                        Some(&format!("field `{field}` on `{table}`")),
                        field,
                        Some(kind),
                        self.schema,
                    ),
                ));
            }
        }
        match kind.as_ref().and_then(record_link_target) {
            Some(linked) => {
                *out_table = Some(linked);
                segments.clear();
            }
            None if kind.is_some() => *segments = path,
            None => *out_table = None,
        }
    }

    /// Walks an idiom's field parts, resolving each against the table in scope
    /// and re-rooting on `record<>` links so linked-table fields type too.
    fn walk_idiom(&mut self, root: Option<&str>, idiom: &ast::Idiom) {
        use ast::IdiomPart;
        // An idiom rooted in a known-kind `$var` (`$direct[0].role`) resolves
        // through value kinds, not the row table.
        if let Some(first) = idiom.parts.first() {
            if let IdiomPart::Start(inner) = &first.node {
                if let ast::Expr::Param(name) = &inner.node {
                    if self.var_kind(name).is_some() {
                        self.resolve_value_idiom(name, inner.span, idiom);
                        return;
                    }
                }
            }
        }
        let mut table = root.map(str::to_string);
        // Field segments accumulated relative to the current `table`.
        let mut segments: Vec<String> = Vec::new();
        for part in &idiom.parts {
            match &part.node {
                IdiomPart::Start(inner) => {
                    self.walk_expr(root, inner);
                    // Rooted in a leading value, not the row table.
                    table = None;
                    segments.clear();
                }
                IdiomPart::Field(name) => {
                    let Some(current) = table.clone() else {
                        continue;
                    };
                    let Some(def) = self.schema.table(&current) else {
                        table = None;
                        continue;
                    };
                    let mut path = segments.clone();
                    path.push(name.clone());
                    let kind = crate::analyzer::data::select::kind_for_path(def, &path);
                    if self.covers(part.span) {
                        if let Some(kind) = &kind {
                            let span = SourceSpan::new(self.source.clone(), part.span);
                            self.out.push((
                                span,
                                symbol_markdown(
                                    Some(&format!("field `{name}` on `{current}`")),
                                    name,
                                    Some(kind),
                                    self.schema,
                                ),
                            ));
                        }
                    }
                    // Advance the scope: follow a record link into its table,
                    // stay on the same table for a nested object, or give up.
                    match kind.as_ref().and_then(record_link_target) {
                        Some(linked) => {
                            table = Some(linked);
                            segments.clear();
                        }
                        None if kind.is_some() => segments = path,
                        None => table = None,
                    }
                }
                IdiomPart::Index(inner) | IdiomPart::Where(inner) => self.walk_expr(root, inner),
                IdiomPart::Method { args, .. } => {
                    for arg in args {
                        self.walk_expr(root, arg);
                    }
                }
                IdiomPart::Graph { step, .. } => {
                    for target in &step.targets {
                        self.table_ref(target);
                    }
                    if let Some(where_clause) = &step.where_clause {
                        self.walk_expr(root, where_clause);
                    }
                    // The shape after a graph step is opaque here.
                    table = None;
                    segments.clear();
                }
                IdiomPart::Destructure(idioms) => {
                    for sub in idioms {
                        self.walk_idiom(table.as_deref(), &sub.node);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Collects go-to-definition targets by walking a freshly-lowered statement
/// tree — the definition twin of [`SchemaHovers`]. Each candidate is a
/// `(reference-span, definition-span)` pair the caller feeds through the
/// smallest-covering `consider` closure. Table/field/`fn::` scope tracking is
/// identical to hover's; only the leaf emission differs (a definition location
/// instead of markdown), and only definitely-declared symbols emit.
struct SchemaDefs<'a> {
    schema: &'a SchemaIndex,
    source: &'a SourceId,
    offset: u32,
    /// `$name` (no sigil) → its binding site, for resolving param/LET uses.
    bindings: &'a std::collections::HashMap<String, SourceSpan>,
    /// Declared `DEFINE FUNCTION` parameters in scope while walking a function
    /// body, by name → the param's name span in the signature. Populated on
    /// entry to the body and restored on exit, so go-to-def on a `$param` use
    /// jumps to its declaration. A `LET`/`DEFINE PARAM` of the same name wins
    /// (`bindings` is checked first).
    param_spans: std::collections::HashMap<String, SourceSpan>,
    out: Vec<(ByteRange, SourceSpan)>,
}

impl SchemaDefs<'_> {
    fn covers(&self, span: ByteRange) -> bool {
        self.offset >= span.start() && self.offset <= span.end()
    }

    /// A table reference → its `DEFINE TABLE` name span, for tables the schema
    /// knows.
    fn table_ref(&mut self, name: &ast::Spanned<String>) {
        if !self.covers(name.span) {
            return;
        }
        if let Some(def) = self.schema.table(&name.node) {
            self.out.push((name.span, def.name_span.clone()));
        }
    }

    fn walk_statement(&mut self, statement: &ast::Spanned<ast::Statement>) {
        use ast::Statement;
        match &statement.node {
            Statement::Select(select) => {
                let root = single_table(&select.from);
                for source in &select.from {
                    self.walk_expr(None, source);
                }
                for projection in &select.projections {
                    if let ast::Projection::Expr { expr, .. } = projection {
                        self.walk_expr(root, expr);
                    }
                }
                if let Some(where_clause) = &select.where_clause {
                    self.walk_expr(root, where_clause);
                }
                if let Some(group) = &select.group {
                    for key in &group.keys {
                        self.walk_idiom(root, &key.node);
                    }
                }
                if let Some(order) = &select.order {
                    for key in &order.keys {
                        self.walk_expr(root, &key.expr);
                    }
                }
                for idiom in select.omit.iter().chain(&select.split).chain(&select.fetch) {
                    self.walk_idiom(root, &idiom.node);
                }
                for extra in [&select.limit, &select.start, &select.timeout].into_iter().flatten() {
                    self.walk_expr(None, extra);
                }
            }
            Statement::Create(create) => {
                let root = single_table(&create.targets);
                for target in &create.targets {
                    self.walk_expr(None, target);
                }
                self.walk_data(root, create.data.as_ref());
            }
            Statement::Update(update) => {
                let root = single_table(&update.targets);
                for target in &update.targets {
                    self.walk_expr(None, target);
                }
                self.walk_data(root, update.data.as_ref());
                if let Some(where_clause) = &update.where_clause {
                    self.walk_expr(root, where_clause);
                }
            }
            Statement::Upsert(upsert) => {
                let root = single_table(&upsert.targets);
                for target in &upsert.targets {
                    self.walk_expr(None, target);
                }
                self.walk_data(root, upsert.data.as_ref());
                if let Some(where_clause) = &upsert.where_clause {
                    self.walk_expr(root, where_clause);
                }
            }
            Statement::Delete(delete) => {
                let root = single_table(&delete.targets);
                for target in &delete.targets {
                    self.walk_expr(None, target);
                }
                if let Some(where_clause) = &delete.where_clause {
                    self.walk_expr(root, where_clause);
                }
            }
            Statement::Insert(insert) => {
                let root = insert
                    .target
                    .as_ref()
                    .and_then(|target| expr_table_name(&target.node));
                if let Some(target) = &insert.target {
                    self.walk_expr(None, target);
                }
                self.walk_insert_data(root, &insert.data);
            }
            Statement::Relate(relate) => {
                for endpoint in [&relate.from, &relate.edge, &relate.to].into_iter().flatten() {
                    self.walk_expr(None, endpoint);
                }
                let root = relate.edge.as_ref().and_then(|edge| expr_table_name(&edge.node));
                self.walk_data(root, relate.data.as_ref());
            }
            Statement::Define(define) => self.walk_define(define),
            Statement::Remove(remove) => match &remove.target {
                ast::RemoveTarget::Table(name) => self.table_ref(name),
                ast::RemoveTarget::Field { field, table } => {
                    self.table_ref(table);
                    self.walk_idiom(Some(table.node.as_str()), &field.node);
                }
                ast::RemoveTarget::Index { table, .. } => self.table_ref(table),
                ast::RemoveTarget::Other(_) => {}
            },
            Statement::Alter(alter) => {
                if let Some(table) = &alter.table {
                    self.table_ref(table);
                }
            }
            Statement::LiveSelect(live) => {
                if let Some(table) = &live.table {
                    self.table_ref(table);
                }
            }
            Statement::Info(info) => {
                if let Some(table) = &info.table {
                    self.table_ref(table);
                }
            }
            Statement::Show(show) => {
                if let Some(table) = &show.table {
                    self.table_ref(table);
                }
            }
            Statement::Rebuild(rebuild) => {
                if let Some(table) = &rebuild.table {
                    self.table_ref(table);
                }
            }
            Statement::Let(let_stmt) => self.walk_expr(None, &let_stmt.value),
            Statement::Return(ret) => {
                if let Some(value) = &ret.value {
                    self.walk_expr(None, value);
                }
            }
            Statement::Throw(throw) => {
                if let Some(value) = &throw.value {
                    self.walk_expr(None, value);
                }
            }
            Statement::Kill(kill) => {
                if let Some(id) = &kill.id {
                    self.walk_expr(None, id);
                }
            }
            Statement::IfElse(if_else) => {
                for branch in &if_else.branches {
                    self.walk_expr(None, &branch.condition);
                    self.walk_block(&branch.body);
                }
                if let Some(else_branch) = &if_else.else_branch {
                    self.walk_block(else_branch);
                }
            }
            Statement::For(for_stmt) => {
                self.walk_expr(None, &for_stmt.iterable);
                self.walk_block(&for_stmt.body);
            }
            Statement::Block(block) => self.walk_block(block),
            Statement::Expr(expr) => self.walk_expr(None, expr),
            _ => {}
        }
    }

    fn walk_define(&mut self, define: &ast::DefineStmt) {
        use ast::DefineStmt;
        match define {
            DefineStmt::Field(field) => {
                self.table_ref(&field.table);
                self.walk_idiom(Some(field.table.node.as_str()), &field.path.node);
                let root = Some(field.table.node.as_str());
                for expr in [&field.default, &field.value, &field.assert].into_iter().flatten() {
                    self.walk_expr(root, expr);
                }
                for predicate in &field.permissions {
                    self.walk_expr(root, predicate);
                }
            }
            DefineStmt::Table(table) => {
                if let Some(relation) = &table.relation {
                    for endpoint in relation.in_tables.iter().chain(&relation.out_tables) {
                        self.table_ref(endpoint);
                    }
                }
                let root = Some(table.name.node.as_str());
                for predicate in &table.permissions {
                    self.walk_expr(root, predicate);
                }
            }
            DefineStmt::Index(index) => {
                self.table_ref(&index.table);
                for idiom in &index.fields {
                    self.walk_idiom(Some(index.table.node.as_str()), &idiom.node);
                }
            }
            DefineStmt::Event(event) => {
                self.table_ref(&event.table);
                let root = Some(event.table.node.as_str());
                for expr in [&event.when, &event.then].into_iter().flatten() {
                    self.walk_expr(root, expr);
                }
            }
            // A DEFINE FUNCTION body is ordinary statement territory: its
            // `LET`/`FOR` var uses, `fn::` calls, and idioms all resolve the
            // same as at top level, so walk it — but first bring the declared
            // parameters into scope so go-to-def on a `$param` use in the body
            // jumps to its declaration in the signature.
            DefineStmt::Function(func) => {
                let saved = std::mem::take(&mut self.param_spans);
                for (name, _ty) in &func.params {
                    self.param_spans.insert(
                        name.node.clone(),
                        SourceSpan::new(self.source.clone(), name.span),
                    );
                }
                if let Some(body) = &func.body {
                    self.walk_block(body);
                }
                self.param_spans = saved;
            }
            _ => {}
        }
    }

    fn walk_block(&mut self, block: &ast::Block) {
        for statement in &block.statements {
            self.walk_statement(statement);
        }
    }

    fn walk_data(&mut self, root: Option<&str>, data: Option<&ast::DataClause>) {
        use ast::DataClause;
        match data {
            Some(DataClause::Set(assignments)) => {
                for assignment in assignments {
                    self.walk_idiom(root, &assignment.target.node);
                    self.walk_expr(root, &assignment.value);
                }
            }
            Some(DataClause::Unset(idioms)) => {
                for idiom in idioms {
                    self.walk_idiom(root, &idiom.node);
                }
            }
            Some(
                DataClause::Content(expr)
                | DataClause::Merge(expr)
                | DataClause::Patch(expr)
                | DataClause::Replace(expr)
                | DataClause::Single(expr),
            ) => self.walk_expr(root, expr),
            _ => {}
        }
    }

    fn walk_insert_data(&mut self, root: Option<&str>, data: &ast::InsertData) {
        use ast::InsertData;
        match data {
            InsertData::Values(exprs) => {
                for expr in exprs {
                    self.walk_expr(root, expr);
                }
            }
            InsertData::Rows { rows, .. } => {
                for row in rows {
                    for (column, value) in row {
                        self.walk_idiom(root, &column.node);
                        self.walk_expr(root, value);
                    }
                }
            }
            InsertData::Assignments(assignments) => {
                for (column, value) in assignments {
                    self.walk_idiom(root, &column.node);
                    self.walk_expr(root, value);
                }
            }
            InsertData::Partial(_) => {}
        }
    }

    fn walk_expr(&mut self, root: Option<&str>, expr: &ast::Spanned<ast::Expr>) {
        use ast::Expr;
        match &expr.node {
            Expr::Table(name) => self.table_ref(name),
            Expr::RecordId { table, .. } => self.table_ref(table),
            Expr::Param(name) => {
                // A `$param` / `LET` variable use → its binding site. A
                // `LET`/`DEFINE PARAM` binding wins over a function parameter
                // of the same name.
                if self.covers(expr.span) {
                    if let Some(target) =
                        self.bindings.get(name).or_else(|| self.param_spans.get(name))
                    {
                        self.out.push((expr.span, target.clone()));
                    }
                }
            }
            Expr::Idiom(idiom) => self.walk_idiom(root, idiom),
            Expr::Binary { lhs, rhs, .. } => {
                self.walk_expr(root, lhs);
                self.walk_expr(root, rhs);
            }
            Expr::Prefix { expr, .. } | Expr::Cast { expr, .. } => self.walk_expr(root, expr),
            Expr::Call(call) => {
                // A `fn::` call → its `DEFINE FUNCTION` name span.
                if self.covers(call.path.span) {
                    if let Some(func) = self.schema.function(&call.path.node) {
                        self.out.push((call.path.span, func.name_span.clone()));
                    }
                }
                for arg in &call.args {
                    self.walk_expr(root, arg);
                }
            }
            Expr::Object(entries) => {
                for (_, value) in entries {
                    self.walk_expr(root, value);
                }
            }
            Expr::Array(items) => {
                for item in items {
                    self.walk_expr(root, item);
                }
            }
            Expr::Subquery(statement) => self.walk_statement(statement),
            Expr::Block(block) => self.walk_block(block),
            Expr::Closure(closure) => self.walk_expr(root, &closure.body),
            _ => {}
        }
    }

    /// Walks an idiom's field parts, resolving each against the table in scope
    /// (re-rooting on `record<>` links exactly as hover does) and emitting the
    /// owning `DEFINE FIELD`'s name span. Only a directly-declared field emits;
    /// object prefixes, implicit `id`/`in`/`out`, and schemaless fields have no
    /// declaration to jump to.
    fn walk_idiom(&mut self, root: Option<&str>, idiom: &ast::Idiom) {
        use ast::IdiomPart;
        let mut table = root.map(str::to_string);
        let mut segments: Vec<String> = Vec::new();
        for part in &idiom.parts {
            match &part.node {
                IdiomPart::Start(inner) => {
                    self.walk_expr(root, inner);
                    table = None;
                    segments.clear();
                }
                IdiomPart::Field(name) => {
                    let Some(current) = table.clone() else {
                        continue;
                    };
                    let Some(def) = self.schema.table(&current) else {
                        table = None;
                        continue;
                    };
                    let mut path = segments.clone();
                    path.push(name.clone());
                    let kind = crate::analyzer::data::select::kind_for_path(def, &path);
                    if self.covers(part.span) {
                        if let Some(field) = def.fields.get(&path.join(".")) {
                            self.out.push((part.span, field.name_span.clone()));
                        }
                    }
                    match kind.as_ref().and_then(record_link_target) {
                        Some(linked) => {
                            table = Some(linked);
                            segments.clear();
                        }
                        None if kind.is_some() => segments = path,
                        None => table = None,
                    }
                }
                IdiomPart::Index(inner) | IdiomPart::Where(inner) => self.walk_expr(root, inner),
                IdiomPart::Method { args, .. } => {
                    for arg in args {
                        self.walk_expr(root, arg);
                    }
                }
                IdiomPart::Graph { step, .. } => {
                    for target in &step.targets {
                        self.table_ref(target);
                    }
                    if let Some(where_clause) = &step.where_clause {
                        self.walk_expr(root, where_clause);
                    }
                    table = None;
                    segments.clear();
                }
                IdiomPart::Destructure(idioms) => {
                    for sub in idioms {
                        self.walk_idiom(table.as_deref(), &sub.node);
                    }
                }
                _ => {}
            }
        }
    }
}

/// The table a set of source/target expressions names, when it is exactly one
/// plain table (or record id). Any other shape — none, several, a subquery —
/// leaves the row table unknown so field paths stay unresolved.
fn single_table(exprs: &[ast::Spanned<ast::Expr>]) -> Option<&str> {
    match exprs {
        [only] => expr_table_name(&only.node),
        _ => None,
    }
}

/// The table name a source/target expression names, if it is a bare table or a
/// record id (`person` / `person:one`).
fn expr_table_name(expr: &ast::Expr) -> Option<&str> {
    match expr {
        ast::Expr::Table(name) => Some(name.node.as_str()),
        ast::Expr::RecordId { table, .. } => Some(table.node.as_str()),
        _ => None,
    }
}

/// The element kind of an array/set (`array<T>` → `T`), unwrapping an
/// `option<array<T>>` to the same. Anything else has no element kind, so a
/// subscript into it resolves to nothing (low-FP).
fn element_kind(kind: &Kind) -> Option<Kind> {
    match kind {
        Kind::Array(inner, _) | Kind::Set(inner, _) => Some((**inner).clone()),
        Kind::Either(variants) => variants
            .iter()
            .filter(|variant| !matches!(variant, Kind::None | Kind::Null))
            .find_map(element_kind),
        _ => None,
    }
}

/// The kind of `field` within a literal-object value kind (`{ role: string }`
/// → `.role` is `string`), unwrapping `option<{...}>`. Records resolve their
/// fields via the schema, not here; a plain `object` is opaque. Returns
/// nothing when the field is absent or the shape carries no field map.
fn field_kind(kind: &Kind, field: &str) -> Option<Kind> {
    match kind {
        Kind::Literal(KindLiteral::Object(entries)) => entries
            .iter()
            .find(|(name, _)| name.as_str() == field)
            .map(|(_, kind)| kind.clone()),
        Kind::Either(variants) => variants
            .iter()
            .filter(|variant| !matches!(variant, Kind::None | Kind::Null))
            .find_map(|variant| field_kind(variant, field)),
        _ => None,
    }
}

/// A `fn::` signature popover:
/// `fn::name($p: kind, ...) -> return`. Built from the `DEFINE FUNCTION`'s
/// declared params and its declared/inferred return kind (the `-> ...` is
/// omitted when the return kind is unknown).
fn function_signature_markdown(func: &crate::schema::FunctionDef) -> String {
    let params: Vec<String> = func
        .args
        .iter()
        .map(|param| {
            let kind = param
                .kind
                .as_ref()
                .map_or_else(|| "any".to_string(), render_kind);
            format!("${}: {kind}", param.name)
        })
        .collect();
    let mut signature = format!("{}({})", func.name, params.join(", "));
    if let Some(return_kind) = &func.return_kind {
        signature.push_str(&format!(" -> {}", render_kind(return_kind)));
    }
    format!("```surql\n{signature}\n```")
}

/// The single linked table a `record<T>` (or `option<record<T>>`) points at,
/// used to re-root idiom traversal across a link. Multi-table or ambiguous
/// links resolve to nothing rather than guess.
fn record_link_target(kind: &Kind) -> Option<String> {
    match kind {
        Kind::Record(tables) => match tables.as_slice() {
            [table] => Some(table.as_str().to_string()),
            _ => None,
        },
        Kind::Either(variants) => {
            let mut records = variants
                .iter()
                .filter(|variant| !matches!(variant, Kind::None | Kind::Null));
            match (records.next(), records.next()) {
                (Some(single), None) => record_link_target(single),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{analyze_workspace, Workspace};
    use surrealdb_types::{KindLiteral, Table};

    fn analyze(text: &str) -> (AnalysisOutput, SchemaIndex, SourceId) {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source("query".into(), text.into());
        let output = analyze_workspace(&workspace);
        let analysis = output.sources[&source].clone();
        (analysis, output.schema, source)
    }

    #[test]
    fn render_kind_compact_forms() {
        assert_eq!(render_kind(&Kind::String), "string");
        assert_eq!(
            render_kind(&Kind::Record(vec![Table::from("file")])),
            "record<file>"
        );
        assert_eq!(
            render_kind(&Kind::Either(vec![Kind::None, Kind::String])),
            "option<string>"
        );
        assert_eq!(
            render_kind(&Kind::Array(Box::new(Kind::String), None)),
            "array<string>"
        );
        assert_eq!(
            render_kind(&Kind::Either(vec![Kind::Int, Kind::String])),
            "int | string"
        );
        let object = Kind::Literal(KindLiteral::Object(
            [("name".to_string(), Kind::String)].into_iter().collect(),
        ));
        assert_eq!(
            render_kind(&Kind::Array(Box::new(object), None)),
            "array<{ name: string }>"
        );
    }

    #[test]
    fn let_binding_hints_expose_inferred_kinds() {
        let (output, _schema, _source) = analyze("LET $x = 1;\nLET $name = 'Ada';");
        let hints = let_binding_hints(&output);

        assert_eq!(hints.len(), 2);
        assert_eq!(hints[0].label, "<int>");
        assert_eq!(hints[1].label, "<string>");
        // The hint anchors on the `$x` token, not the whole statement.
        assert_eq!(hints[0].name_span.range().start(), 4);
    }

    #[test]
    fn hover_resolves_a_let_binding_kind() {
        let text = "LET $age = 42;";
        let (output, schema, source) = analyze(text);
        // Cursor on the `$age` token (offset 6 is inside `$age`).
        let hover = hover_at(&output, &schema, &source, text, 6).expect("hover over $age");

        assert!(hover.markdown.contains("$age: int"));
        assert_eq!(hover.span.range().start(), 4);
    }

    #[test]
    fn hover_resolves_a_param_use_kind() {
        // `$id` is compared against the int field `age`, so it constrains
        // to int.
        let text = "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nSELECT * FROM person WHERE age = $id;";
        let (output, schema, source) = analyze(text);
        let offset = text.find("$id").expect("param present") as u32 + 1;
        let hover = hover_at(&output, &schema, &source, text, offset).expect("hover over $id");

        assert!(hover.markdown.contains("$id: int"), "got: {}", hover.markdown);
    }

    #[test]
    fn hover_over_a_table_name_lists_its_fields() {
        let text = "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;";
        let (output, schema, source) = analyze(text);
        // Cursor on the `person` name in the DEFINE TABLE statement.
        let offset = text.find("person").expect("table name present") as u32 + 1;
        let hover = hover_at(&output, &schema, &source, text, offset).expect("hover over table");

        assert!(hover.markdown.contains("DEFINE TABLE person"), "got: {}", hover.markdown);
        assert!(hover.markdown.contains("name: string"), "got: {}", hover.markdown);
        assert!(hover.markdown.contains("age: int"), "got: {}", hover.markdown);
    }

    #[test]
    fn hover_over_nothing_typed_returns_none() {
        let text = "LET $age = 42;";
        let (output, schema, source) = analyze(text);
        // Offset 0 is on the `LET` keyword.
        assert!(hover_at(&output, &schema, &source, text, 0).is_none());
    }

    #[test]
    fn hover_resolves_value_context_param_in_a_define_field_assert() {
        let text = "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD status ON t TYPE string ASSERT $value IN ['a', 'b'];";
        let (output, schema, source) = analyze(text);
        // The `$value` in the ASSERT clause takes the field's declared kind.
        let offset = text.find("$value").expect("param present") as u32 + 1;
        let hover = hover_at(&output, &schema, &source, text, offset).expect("hover over $value");

        assert!(hover.markdown.contains("$value: string"), "got: {}", hover.markdown);
    }

    #[test]
    fn hover_resolves_event_and_after_context_params_in_a_define_event() {
        let text = "DEFINE TABLE t SCHEMAFULL;\n\
             DEFINE FIELD name ON t TYPE string;\n\
             DEFINE EVENT ev ON t WHEN $event = 'CREATE' THEN { UPDATE t SET name = $after.name; };";
        let (output, schema, source) = analyze(text);

        let event_offset = text.find("$event").expect("$event present") as u32 + 1;
        let event_hover =
            hover_at(&output, &schema, &source, text, event_offset).expect("hover over $event");
        assert!(
            event_hover.markdown.contains("'CREATE' | 'UPDATE' | 'DELETE'"),
            "got: {}",
            event_hover.markdown
        );

        let after_offset = text.find("$after").expect("$after present") as u32 + 1;
        let after_hover =
            hover_at(&output, &schema, &source, text, after_offset).expect("hover over $after");
        assert!(
            after_hover.markdown.contains("$after: record<t>"),
            "got: {}",
            after_hover.markdown
        );
    }

    #[test]
    fn hover_over_a_table_reference_in_from_lists_its_fields() {
        let text = "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             DEFINE FIELD age ON person TYPE int;\n\
             SELECT * FROM person;";
        let (output, schema, source) = analyze(text);
        // Cursor on the `person` in the `FROM person` clause (last occurrence).
        let offset = text.rfind("person").expect("FROM table present") as u32 + 1;
        let hover = hover_at(&output, &schema, &source, text, offset).expect("hover over FROM table");

        assert!(hover.markdown.contains("DEFINE TABLE person"), "got: {}", hover.markdown);
        assert!(hover.markdown.contains("name: string"), "got: {}", hover.markdown);
        assert!(hover.markdown.contains("age: int"), "got: {}", hover.markdown);
    }

    #[test]
    fn hover_over_a_projected_field_shows_its_type() {
        let text = "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD age ON person TYPE int;\n\
             SELECT age FROM person;";
        let (output, schema, source) = analyze(text);
        // Cursor on the projected `age`, not the DEFINE FIELD name.
        let offset = text.rfind("age").expect("projected field present") as u32 + 1;
        let hover =
            hover_at(&output, &schema, &source, text, offset).expect("hover over projected field");

        assert!(hover.markdown.contains("age: int"), "got: {}", hover.markdown);
    }

    #[test]
    fn hover_over_a_record_link_field_resolves_it_and_the_linked_field() {
        let text = "DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE FIELD name ON user TYPE string;\n\
             DEFINE TABLE post SCHEMAFULL;\n\
             DEFINE FIELD author ON post TYPE record<user>;\n\
             SELECT author.name FROM post;";
        let (output, schema, source) = analyze(text);

        // The link field itself renders as `record<user>`.
        let author_offset = text.rfind("author").expect("projected link present") as u32 + 1;
        let author_hover =
            hover_at(&output, &schema, &source, text, author_offset).expect("hover over link field");
        assert!(
            author_hover.markdown.contains("author: record<user>"),
            "got: {}",
            author_hover.markdown
        );

        // The field reached through the link resolves against the linked table.
        let name_offset = text.rfind("name").expect("linked field present") as u32 + 1;
        let name_hover =
            hover_at(&output, &schema, &source, text, name_offset).expect("hover over linked field");
        assert!(
            name_hover.markdown.contains("name: string"),
            "got: {}",
            name_hover.markdown
        );
    }

    #[test]
    fn hover_over_a_field_on_a_schemaless_table_returns_no_misleading_type() {
        // Low-FP: an undeclared field on a schemaless table has no known type,
        // so hovering it must not invent one.
        let text = "DEFINE TABLE person;\nSELECT nickname FROM person;";
        let (output, schema, source) = analyze(text);
        let offset = text.rfind("nickname").expect("field present") as u32 + 1;
        assert!(hover_at(&output, &schema, &source, text, offset).is_none());
    }

    #[test]
    fn definition_of_a_from_table_points_at_its_define_table_name() {
        let text = "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             SELECT name FROM person;";
        let (output, schema, source) = analyze(text);
        // Cursor on the `person` in `FROM person` (last occurrence).
        let offset = text.rfind("person").expect("FROM table present") as u32 + 1;
        let target =
            definition_at(&output, &schema, &source, text, offset).expect("definition of table");

        let expected = text.find("person").expect("DEFINE TABLE name present") as u32;
        assert_eq!(target.span.source(), &source);
        assert_eq!(target.span.range().start(), expected);
    }

    #[test]
    fn definition_of_a_projected_field_points_at_its_define_field_name() {
        let text = "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             SELECT name FROM person;";
        let (output, schema, source) = analyze(text);
        // Cursor on the projected `name`.
        let offset = text.rfind("name").expect("projected field present") as u32 + 1;
        let target =
            definition_at(&output, &schema, &source, text, offset).expect("definition of field");

        // Jumps to the `name` in the DEFINE FIELD statement.
        let expected = text.find("name ON person").expect("DEFINE FIELD name present") as u32;
        assert_eq!(target.span.source(), &source);
        assert_eq!(target.span.range().start(), expected);
    }

    #[test]
    fn definition_of_a_linked_field_follows_the_record_link() {
        let text = "DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE FIELD name ON user TYPE string;\n\
             DEFINE TABLE post SCHEMAFULL;\n\
             DEFINE FIELD author ON post TYPE record<user>;\n\
             SELECT author.name FROM post;";
        let (output, schema, source) = analyze(text);
        // Cursor on the `name` reached through the `author` record link.
        let offset = text.rfind("name").expect("linked field present") as u32 + 1;
        let target = definition_at(&output, &schema, &source, text, offset)
            .expect("definition of linked field");

        let expected = text.find("name ON user").expect("DEFINE FIELD name present") as u32;
        assert_eq!(target.span.range().start(), expected);
    }

    #[test]
    fn definition_of_a_function_call_points_at_its_define_function() {
        let text = "DEFINE FUNCTION fn::greet($who: string) -> string { RETURN $who; };\n\
             RETURN fn::greet('ada');";
        let (output, schema, source) = analyze(text);
        let offset = text.rfind("fn::greet").expect("call present") as u32 + 2;
        let target = definition_at(&output, &schema, &source, text, offset)
            .expect("definition of function");

        let expected = text.find("fn::greet").expect("DEFINE FUNCTION name present") as u32;
        assert_eq!(target.span.range().start(), expected);
    }

    #[test]
    fn definition_of_a_let_variable_use_points_at_its_binding() {
        let text = "LET $x = 42;\nRETURN $x + 1;";
        let (output, schema, source) = analyze(text);
        let offset = text.rfind("$x").expect("use present") as u32 + 1;
        let target =
            definition_at(&output, &schema, &source, text, offset).expect("definition of let var");

        let expected = text.find("$x").expect("binding present") as u32;
        assert_eq!(target.span.range().start(), expected);
    }

    #[test]
    fn definition_of_a_schemaless_field_returns_none() {
        // Low-FP: an undeclared field on a schemaless table has no DEFINE to
        // jump to.
        let text = "DEFINE TABLE person;\nSELECT nickname FROM person;";
        let (output, schema, source) = analyze(text);
        let offset = text.rfind("nickname").expect("field present") as u32 + 1;
        assert!(definition_at(&output, &schema, &source, text, offset).is_none());
    }

    #[test]
    fn definition_resolves_a_field_across_files() {
        // The schema lives in one source; the query that references it in
        // another. The definition target must point back into the schema
        // source, not the query source.
        let mut workspace = Workspace::default();
        let schema_source = workspace.add_virtual_source(
            "schema".into(),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;".into(),
        );
        let query_text = "SELECT name FROM person;";
        let query_source = workspace.add_virtual_source("query".into(), query_text.into());
        let output = analyze_workspace(&workspace);
        let analysis = output.sources[&query_source].clone();

        // Field reference → DEFINE FIELD in the schema source.
        let field_offset = query_text.find("name").expect("field present") as u32 + 1;
        let field_target =
            definition_at(&analysis, &output.schema, &query_source, query_text, field_offset)
                .expect("cross-file field definition");
        assert_eq!(field_target.span.source(), &schema_source);

        // Table reference → DEFINE TABLE in the schema source.
        let table_offset = query_text.find("person").expect("table present") as u32 + 1;
        let table_target =
            definition_at(&analysis, &output.schema, &query_source, query_text, table_offset)
                .expect("cross-file table definition");
        assert_eq!(table_target.span.source(), &schema_source);
        assert_eq!(table_target.span.range().start(), 13);
    }

    #[test]
    fn hover_over_value_outside_a_define_construct_returns_none() {
        // `$value` is only a context param inside a DEFINE body; a bare use
        // elsewhere is an ordinary (here unknown) param, not a context one.
        let text = "RETURN $value + 1;";
        let (output, schema, source) = analyze(text);
        let offset = text.find("$value").expect("param present") as u32 + 1;
        let hover = hover_at(&output, &schema, &source, text, offset);
        // If anything resolves it must not claim the context-param field kind.
        if let Some(hover) = hover {
            assert!(!hover.markdown.contains("record<"), "got: {}", hover.markdown);
        }
    }

    /// The DEFINE FUNCTION whose body drives the body-level tests below:
    /// `employee_of` rows expose `role`/`unit`, and `fn::org::unit` returns a
    /// `record<unit>`.
    const BODY_FIXTURE: &str = "DEFINE TABLE employee_of SCHEMAFULL;\n\
         DEFINE FIELD role ON employee_of TYPE string;\n\
         DEFINE FIELD unit ON employee_of TYPE record<unit>;\n\
         DEFINE TABLE unit SCHEMAFULL;\n\
         DEFINE FIELD label ON unit TYPE string;\n\
         DEFINE FUNCTION fn::org::unit($organization: record<unit>) -> record<unit> {\n\
             RETURN $organization;\n\
         };\n\
         DEFINE FUNCTION fn::org::report($organization: record<unit>, $auth: record<unit>) {\n\
             LET $direct = SELECT role, unit FROM employee_of WHERE unit = $organization;\n\
             LET $u = fn::org::unit($organization);\n\
             RETURN $direct[0].role;\n\
         };";

    #[test]
    fn hover_resolves_a_body_level_let_binding_and_its_use() {
        let (output, schema, source) = analyze(BODY_FIXTURE);

        // Binding site: `LET $direct = SELECT role, unit FROM ...` → the
        // SELECT's row shape as an array of `{ role, unit }`.
        let bind_offset = BODY_FIXTURE.find("$direct").expect("binding present") as u32 + 1;
        let bind = hover_at(&output, &schema, &source, BODY_FIXTURE, bind_offset)
            .expect("hover over body LET binding");
        assert!(bind.markdown.contains("$direct: array<"), "got: {}", bind.markdown);
        assert!(bind.markdown.contains("role: string"), "got: {}", bind.markdown);

        // Use site: `$direct[0].role` — the `$direct` token resolves to the
        // same array kind.
        let use_offset = BODY_FIXTURE.rfind("$direct").expect("use present") as u32 + 1;
        let used = hover_at(&output, &schema, &source, BODY_FIXTURE, use_offset)
            .expect("hover over body LET use");
        assert!(used.markdown.contains("$direct: array<"), "got: {}", used.markdown);
    }

    #[test]
    fn hover_over_a_subscript_idiom_resolves_the_element_field() {
        let (output, schema, source) = analyze(BODY_FIXTURE);
        // `$direct[0].role`: index into the array element `{ role, unit }`,
        // then `.role` is `string`.
        let role_use = BODY_FIXTURE.rfind(".role").expect("subscript present") as u32 + 1;
        let hover = hover_at(&output, &schema, &source, BODY_FIXTURE, role_use)
            .expect("hover over subscript field");
        assert!(hover.markdown.contains("role: string"), "got: {}", hover.markdown);
    }

    #[test]
    fn hover_over_a_function_call_shows_its_signature() {
        let (output, schema, source) = analyze(BODY_FIXTURE);
        // The `fn::org::unit(...)` call in the body → its declared signature.
        let call = BODY_FIXTURE
            .find("fn::org::unit($organization)")
            .expect("call present") as u32
            + 2;
        let hover =
            hover_at(&output, &schema, &source, BODY_FIXTURE, call).expect("hover over fn call");
        assert!(
            hover.markdown.contains("fn::org::unit($organization: record<unit>) -> record<unit>"),
            "got: {}",
            hover.markdown
        );
    }

    #[test]
    fn inlay_hints_cover_body_level_lets() {
        let (output, _schema, _source) = analyze(BODY_FIXTURE);
        let hints = let_binding_hints(&output);
        // Both body LETs (`$direct`, `$u`) get a hint; the SELECT one is an
        // array, the call one a record.
        let labels: Vec<&str> = hints.iter().map(|hint| hint.label.as_str()).collect();
        assert!(
            labels.iter().any(|label| label.starts_with("<array<")),
            "expected an array hint, got: {labels:?}"
        );
        assert!(
            labels.iter().any(|label| label.contains("record<unit>")),
            "expected the fn-call record hint, got: {labels:?}"
        );
    }

    #[test]
    fn inlay_hints_include_for_loop_variables_and_skip_any() {
        let text = "DEFINE FUNCTION fn::each($xs: array<int>) {\n\
             FOR $item IN $xs { RETURN $item; };\n\
         };";
        let (output, _schema, _source) = analyze(text);
        let hints = let_binding_hints(&output);
        // `$item` is the element kind `int` of `array<int>`.
        assert!(
            hints.iter().any(|hint| hint.label == "<int>"),
            "expected the FOR var hint, got: {:?}",
            hints.iter().map(|h| &h.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn definition_of_a_body_level_let_use_points_at_its_binding() {
        let (output, schema, source) = analyze(BODY_FIXTURE);
        // Go-to-def on the `$direct` use jumps to its body `LET` binding.
        let use_offset = BODY_FIXTURE.rfind("$direct").expect("use present") as u32 + 1;
        let target = definition_at(&output, &schema, &source, BODY_FIXTURE, use_offset)
            .expect("definition of body let var");
        let expected = BODY_FIXTURE.find("$direct").expect("binding present") as u32;
        assert_eq!(target.span.range().start(), expected);
    }

    #[test]
    fn inlay_label_elides_an_over_long_object_kind() {
        // A wide literal-object kind is truncated with `…` so it never
        // dominates the line.
        let wide = Kind::Literal(KindLiteral::Object(
            (0..12)
                .map(|i| (format!("field_number_{i}"), Kind::String))
                .collect(),
        ));
        assert!(render_kind(&wide).chars().count() > INLAY_LABEL_MAX);
        let elided = elide_label(&render_kind(&wide));
        assert!(elided.ends_with('…'), "got: {elided}");
        assert!(elided.chars().count() <= INLAY_LABEL_MAX + 1);
    }

    /// A RELATION-edge schema for the coverage tests: `has_subsidiary` links
    /// `organization` → `organization`, with two ordinary fields.
    const RELATION_FIXTURE: &str = "DEFINE TABLE organization SCHEMAFULL;\n\
         DEFINE TABLE has_subsidiary SCHEMAFULL TYPE RELATION IN organization OUT organization;\n\
         DEFINE FIELD public ON has_subsidiary TYPE bool;\n\
         DEFINE FIELD status ON has_subsidiary TYPE string;\n\
         DEFINE FUNCTION fn::f($organization: record<organization>) {\n\
             LET $x = SELECT * FROM has_subsidiary WHERE out = $organization;\n\
         };";

    #[test]
    fn table_hover_renders_a_define_fence_with_fields() {
        let (output, schema, source) = analyze(RELATION_FIXTURE);
        let offset = RELATION_FIXTURE
            .find("has_subsidiary TYPE")
            .expect("table decl present") as u32
            + 1;
        let hover = hover_at(&output, &schema, &source, RELATION_FIXTURE, offset)
            .expect("hover over relation table");
        assert!(hover.markdown.contains("```surql"), "got: {}", hover.markdown);
        assert!(
            hover.markdown.contains("DEFINE TABLE has_subsidiary SCHEMAFULL;"),
            "got: {}",
            hover.markdown
        );
        assert!(
            hover.markdown.contains("TYPE RELATION IN organization OUT organization"),
            "got: {}",
            hover.markdown
        );
        assert!(hover.markdown.contains("public: bool"), "got: {}", hover.markdown);
        assert!(hover.markdown.contains("status: string"), "got: {}", hover.markdown);
    }

    #[test]
    fn hover_resolves_a_function_param_at_a_body_use_site() {
        let (output, schema, source) = analyze(RELATION_FIXTURE);
        // `$organization` used deep in the WHERE clause of the body SELECT.
        let use_offset = RELATION_FIXTURE
            .find("= $organization")
            .expect("param use present") as u32
            + 3;
        let hover = hover_at(&output, &schema, &source, RELATION_FIXTURE, use_offset)
            .expect("hover over fn param use");
        assert!(
            hover.markdown.contains("$organization: record<organization>"),
            "got: {}",
            hover.markdown
        );
    }

    #[test]
    fn definition_of_a_function_param_use_jumps_to_the_signature() {
        let (output, schema, source) = analyze(RELATION_FIXTURE);
        let use_offset = RELATION_FIXTURE
            .find("= $organization")
            .expect("param use present") as u32
            + 3;
        let target = definition_at(&output, &schema, &source, RELATION_FIXTURE, use_offset)
            .expect("definition of fn param");
        let expected = RELATION_FIXTURE
            .find("$organization: record")
            .expect("param decl present") as u32;
        assert_eq!(target.span.range().start(), expected);
    }

    #[test]
    fn let_binding_shadows_a_function_param_of_the_same_name() {
        // A body `LET $organization = ...` wins over the function parameter of
        // the same name at a later use site.
        let text = "DEFINE TABLE org SCHEMAFULL;\n\
             DEFINE FUNCTION fn::f($organization: record<org>) {\n\
                 LET $organization = 42;\n\
                 RETURN $organization;\n\
             };";
        let (output, schema, source) = analyze(text);
        let use_offset = text.rfind("$organization").expect("use present") as u32 + 1;
        let hover =
            hover_at(&output, &schema, &source, text, use_offset).expect("hover over shadowed var");
        assert!(hover.markdown.contains("$organization: int"), "got: {}", hover.markdown);
    }

    #[test]
    fn hover_resolves_an_implicit_relation_out_field() {
        let (output, schema, source) = analyze(RELATION_FIXTURE);
        // `out` in `WHERE out = $organization` on the relation table.
        let out_offset = RELATION_FIXTURE
            .find("WHERE out")
            .expect("out present") as u32
            + 6;
        let hover = hover_at(&output, &schema, &source, RELATION_FIXTURE, out_offset)
            .expect("hover over implicit out field");
        assert!(
            hover.markdown.contains("out: record<organization>"),
            "got: {}",
            hover.markdown
        );
    }

    #[test]
    fn definition_of_an_implicit_relation_field_returns_none() {
        let (output, schema, source) = analyze(RELATION_FIXTURE);
        let out_offset = RELATION_FIXTURE
            .find("WHERE out")
            .expect("out present") as u32
            + 6;
        assert!(definition_at(&output, &schema, &source, RELATION_FIXTURE, out_offset).is_none());
    }

}
