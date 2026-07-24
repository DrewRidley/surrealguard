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
    /// The rendered label, already prefixed with `": "`.
    pub label: String,
}

/// Inferred-type inlay hints for every `LET` binding whose kind analysis
/// could determine. Bindings with an undeterminable kind produce no hint.
pub fn let_binding_hints(output: &AnalysisOutput) -> Vec<TypeHint> {
    output
        .statements
        .iter()
        .filter_map(|statement| {
            let binding = statement.let_binding.as_ref()?;
            let kind = binding.kind.as_ref()?;
            Some(TypeHint {
                name_span: binding.name_span.clone(),
                label: format!(": {}", render_kind(kind)),
            })
        })
        .collect()
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

    // `LET $x` binding sites.
    for statement in &output.statements {
        if let Some(binding) = &statement.let_binding {
            consider(
                &binding.name_span,
                symbol_markdown(&format!("${}", binding.name), binding.kind.as_ref(), schema),
            );
        }
    }

    // `$param` use sites.
    for param in &output.inferred_params {
        let markdown = symbol_markdown(&format!("${}", param.name), param.kind.as_ref(), schema);
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
                consider(&span, symbol_markdown(&format!("${name}"), Some(kind), schema));
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

/// Markdown for a variable/parameter symbol: a fenced type line plus,
/// when the kind resolves to records or a literal object, its field list.
fn symbol_markdown(name: &str, kind: Option<&Kind>, schema: &SchemaIndex) -> String {
    let rendered = kind.map_or_else(|| "unknown".to_string(), render_kind);
    let mut markdown = format!("```surql\n{name}: {rendered}\n```");
    if let Some(kind) = kind {
        let fields = field_lines(kind, schema);
        if !fields.is_empty() {
            markdown.push_str("\n\n**fields**\n");
            for line in fields {
                markdown.push_str(&format!("- `{line}`\n"));
            }
        }
    }
    markdown
}

/// Markdown for a table declaration: its record kind plus its field list.
fn table_markdown(table: &crate::schema::TableDef, schema: &SchemaIndex) -> String {
    let mut markdown = format!("```surql\ntable {}\n```", table.name);
    let fields = table_field_lines(&table.name, schema);
    if !fields.is_empty() {
        markdown.push_str("\n\n**fields**\n");
        for line in fields {
            markdown.push_str(&format!("- `{line}`\n"));
        }
    }
    markdown
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
    out: Vec<(SourceSpan, String)>,
}

impl SchemaHovers<'_> {
    fn covers(&self, span: ByteRange) -> bool {
        self.offset >= span.start() && self.offset <= span.end()
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
            Expr::Idiom(idiom) => self.walk_idiom(root, idiom),
            Expr::Binary { lhs, rhs, .. } => {
                self.walk_expr(root, lhs);
                self.walk_expr(root, rhs);
            }
            Expr::Prefix { expr, .. } | Expr::Cast { expr, .. } => self.walk_expr(root, expr),
            Expr::Call(call) => {
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
    /// and re-rooting on `record<>` links so linked-table fields type too.
    fn walk_idiom(&mut self, root: Option<&str>, idiom: &ast::Idiom) {
        use ast::IdiomPart;
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
                            self.out.push((span, symbol_markdown(name, Some(kind), self.schema)));
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
        assert_eq!(hints[0].label, ": int");
        assert_eq!(hints[1].label, ": string");
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

        assert!(hover.markdown.contains("table person"), "got: {}", hover.markdown);
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

        assert!(hover.markdown.contains("table person"), "got: {}", hover.markdown);
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
}
