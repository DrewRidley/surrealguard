//! Read-only queries over analysis output for editor features (inlay
//! hints, hover). This module produces no diagnostics and mutates no
//! state: it only reads the facts the analysis pipeline already computed
//! (`AnalysisOutput`, `SchemaIndex`) and shapes them for presentation.

use surrealdb_types::{Kind, KindLiteral};
use surrealql_analyzer_syntax::ast;
use surrealql_analyzer_syntax::ast::visit::{self, Visitor};
use surrealql_analyzer_syntax::lower::lower_statements;
use surrealql_analyzer_syntax::parse::{parse_source, ParsedSource};
use surrealql_analyzer_syntax::source::SourceId;
use surrealql_analyzer_syntax::span::{ByteRange, SourceSpan};

use crate::analysis::{AnalysisOutput, NarrowingAnalysis};
use crate::analyzer::expression::infer::collection_element_kind;
use crate::render::{render, render_kind, KindContext};
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

/// The character budget an inlay label's rendered kind must fit in. Keeps a
/// long object/union kind from dominating the line; the renderer spends the
/// budget structurally ([`KindContext::Glance`]), so an over-long kind comes
/// back as a shorter *type* rather than a cut string.
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
    let mut seen: std::collections::HashSet<(SourceId, u32, u32)> =
        std::collections::HashSet::new();
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
                label: format!("<{}>", glance_label(kind)),
            })
        })
        .collect()
}

/// Inferred return-type ghosts for `DEFINE FUNCTION`s with no explicit `-> T`.
///
/// Emits a grey `-> <kind>` right after the parameter list's `)` (or `-> none`
/// for a unit body), mirroring rust-analyzer's inferred return hint. A function
/// that already declares `-> T` gets no ghost (it's written), and an
/// undeterminable (`Any`) return produces none either — matching the
/// low-noise policy of [`let_binding_hints`]. The return kind is read from the
/// schema's `FunctionDef::inferred_return` (now resolved even cross-source).
pub fn function_return_hints(text: &str, source: &SourceId, schema: &SchemaIndex) -> Vec<TypeHint> {
    let Ok(parsed) = parse_source(source.clone(), text) else {
        return Vec::new();
    };
    function_return_hints_parsed(&parsed, schema)
}

/// [`function_return_hints`] over an already-parsed source — for a caller that
/// holds the document's [`ParsedSource`] (the LSP does), so a request does not
/// re-parse the whole document.
pub fn function_return_hints_parsed(parsed: &ParsedSource, schema: &SchemaIndex) -> Vec<TypeHint> {
    function_return_hints_lowered(
        &lower_statements(parsed),
        parsed.text(),
        parsed.source_id(),
        schema,
    )
}

/// [`function_return_hints`] over already-lowered statements. `text` is the
/// source they were lowered from: the ghost anchors lexically on the `)` that
/// closes each parameter list.
pub fn function_return_hints_lowered(
    statements: &[ast::Spanned<ast::Statement>],
    text: &str,
    source: &SourceId,
    schema: &SchemaIndex,
) -> Vec<TypeHint> {
    let mut hints = Vec::new();
    for statement in statements {
        let ast::Statement::Define(ast::DefineStmt::Function(def)) = &statement.node else {
            continue;
        };
        // Only a body with no written return type earns a ghost.
        if def.return_ty.is_some() || def.body.is_none() {
            continue;
        }
        // `inferred_return` is absent when the body is `Any` — suppress (no
        // information worth an inline annotation); `Some(none)` is the unit body.
        let Some(kind) = schema
            .function(&def.name.node)
            .and_then(|function| function.inferred_return.clone())
        else {
            continue;
        };
        // Anchor right after the parameters' closing `)`: scan from the last
        // param's end (or the name, for a no-arg function) to the first `)`.
        let search_from = def.params.last().map_or_else(
            || def.name.span.end(),
            |(name, ty)| ty.as_ref().map_or(name.span.end(), |ty| ty.span.end()),
        ) as usize;
        let Some(relative) = text.get(search_from..).and_then(|rest| rest.find(')')) else {
            continue;
        };
        let close = (search_from + relative + 1) as u32;
        let Ok(range) = ByteRange::new(close.saturating_sub(1), close) else {
            continue;
        };
        let label = if matches!(kind, Kind::None) {
            "-> none".to_string()
        } else {
            format!("-> <{}>", glance_label(&kind))
        };
        hints.push(TypeHint {
            name_span: SourceSpan::new(source.clone(), range),
            label,
        });
    }
    hints
}

/// The inlay label for `kind`: its rendered type within [`INLAY_LABEL_MAX`]
/// characters.
///
/// The budget belongs to the renderer, not to a string truncator. A truncator
/// only knows where character 48 falls, which is how
/// `option<string | int | datetime | uuid | decim…` reached the editor — not a
/// type, not parseable, cut mid-name. [`KindContext::Glance`] spends the same
/// budget on the kind instead, widening whole members and object bodies until
/// the render fits, so the label is always a valid type that every value the
/// binding can hold still satisfies.
fn glance_label(kind: &Kind) -> String {
    render(
        kind,
        KindContext::Glance {
            budget: INLAY_LABEL_MAX,
        },
    )
    .text
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

/// The kind a binding (or `param.field.field` path) has **at `offset`**: the
/// innermost flow narrowing whose region covers the cursor, or `None` when no
/// guard is in force there and the declared kind stands.
///
/// Regions nest (a branch body inside a post-guard remainder), so the smallest
/// covering region is the one that holds — the same "smallest covering symbol"
/// rule hover uses everywhere else.
fn narrowed_kind_at<'a>(
    narrowings: &'a [NarrowingAnalysis],
    source: &SourceId,
    path: &str,
    offset: u32,
) -> Option<&'a Kind> {
    narrowing_at(narrowings, source, path, offset).map(|narrowing| &narrowing.kind)
}

/// The narrowing in force for `path` at `offset` — the innermost region
/// covering it — carrying both the kind and the claim that proved it.
pub(crate) fn narrowing_at<'a>(
    narrowings: &'a [NarrowingAnalysis],
    source: &SourceId,
    path: &str,
    offset: u32,
) -> Option<&'a NarrowingAnalysis> {
    narrowings
        .iter()
        .filter(|narrowing| narrowing.path == path && narrowing.span.source() == source)
        .filter(|narrowing| {
            let range = narrowing.span.range();
            offset >= range.start() && offset <= range.end()
        })
        .min_by_key(|narrowing| {
            let range = narrowing.span.range();
            range.end().saturating_sub(range.start())
        })
}

/// Resolves a hover at byte `offset` in `source`: maps the cursor to the
/// smallest covering symbol among the `LET` bindings, parameter uses, table
/// declarations, and the context params (`$value`/`$event`/`$after`/...)
/// bound by an enclosing DEFINE construct, and renders its inferred type.
/// `None` when the cursor is over nothing typed. `text` is the source's full
/// text, needed to locate the enclosing DEFINE construct for context params.
///
/// A hover answers **per occurrence**, not per binding: where a guard has
/// flow-narrowed the symbol under the cursor, the narrowed kind is shown, and
/// at or before that guard the declared kind is. The regions come from
/// [`AnalysisOutput::narrowings`], recorded during analysis — hover reads the
/// cache and never re-analyzes.
pub fn hover_at(
    output: &AnalysisOutput,
    schema: &SchemaIndex,
    source: &SourceId,
    text: &str,
    offset: u32,
) -> Option<HoverInfo> {
    // A source that fails to parse still answers from the analysis facts
    // (bindings, params, tables, context params); only the schema-reference
    // walk needs the tree.
    let statements = parse_source(source.clone(), text)
        .map(|parsed| lower_statements(&parsed))
        .unwrap_or_default();
    hover_at_lowered(output, schema, source, text, &statements, offset)
}

/// [`hover_at`] over an already-parsed source — for a caller that holds the
/// document's [`ParsedSource`] (the LSP does), so a request does not re-parse
/// the whole document.
pub fn hover_at_parsed(
    output: &AnalysisOutput,
    schema: &SchemaIndex,
    parsed: &ParsedSource,
    offset: u32,
) -> Option<HoverInfo> {
    hover_at_lowered(
        output,
        schema,
        parsed.source_id(),
        parsed.text(),
        &lower_statements(parsed),
        offset,
    )
}

/// [`hover_at`] over already-lowered statements. `text` is still needed: the
/// context params bound by an enclosing DEFINE construct are located
/// lexically.
pub fn hover_at_lowered(
    output: &AnalysisOutput,
    schema: &SchemaIndex,
    source: &SourceId,
    text: &str,
    statements: &[ast::Spanned<ast::Statement>],
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
                KindContext::Declared,
                schema,
            ),
        );
    }

    // The kind of each `LET`/`FOR` variable, by name, for resolving `$var`
    // uses and `$var[i].field` idioms anywhere in the source. A name bound
    // more than once keeps the last binding's kind (source-order shadowing).
    let mut let_kinds: std::collections::HashMap<String, Kind> = std::collections::HashMap::new();
    for binding in &output.let_bindings {
        if let Some(kind) = &binding.kind {
            let_kinds.insert(binding.name.clone(), kind.clone());
        }
    }

    // `$param` use sites. Each use is rendered on its own: a guard earlier in
    // the source narrows the uses that follow it, not the ones before it.
    for param in &output.inferred_params {
        for span in &param.spans {
            let narrowed = narrowing_at(&output.narrowings, source, &param.name, offset);
            let here = narrowed
                .map(|narrowing| &narrowing.kind)
                .or(param.kind.as_ref());
            consider(
                span,
                symbol_markdown(
                    Some(&format!("parameter `${}`", param.name)),
                    &format!("${}", param.name),
                    here,
                    KindContext::Occurrence {
                        proved: narrowed.and_then(|narrowing| narrowing.by.as_deref()),
                    },
                    schema,
                ),
            );
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
                        KindContext::Declared,
                        schema,
                    ),
                );
            }
        }
    }

    // Table references and field names in ordinary positions. The analyzed
    // statements carry no per-reference spans, so — as with context params —
    // the lowered tree is walked to locate the identifier under the cursor and
    // resolve it against the schema. Only definitely-typed references produce
    // a hover; anything uncertain is left untouched (low-FP).
    let mut collector = SchemaHovers {
        schema,
        source,
        offset,
        let_kinds: &let_kinds,
        narrowings: &output.narrowings,
        param_kinds: std::collections::HashMap::new(),
        root: None,
        out: Vec::new(),
    };
    for statement in statements {
        collector.visit_statement(statement);
    }
    for (span, markdown) in collector.out {
        consider(&span, markdown);
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
    // Every target comes from walking the tree, so an unparseable source has
    // nothing to offer; `LET` binding sites are still keyed against it.
    let statements = parse_source(source.clone(), text)
        .map(|parsed| lower_statements(&parsed))
        .unwrap_or_default();
    definition_at_lowered(output, schema, source, &statements, offset)
}

/// [`definition_at`] over an already-parsed source — for a caller that holds
/// the document's [`ParsedSource`] (the LSP does), so a request does not
/// re-parse the whole document.
pub fn definition_at_parsed(
    output: &AnalysisOutput,
    schema: &SchemaIndex,
    parsed: &ParsedSource,
    offset: u32,
) -> Option<DefinitionTarget> {
    definition_at_lowered(
        output,
        schema,
        parsed.source_id(),
        &lower_statements(parsed),
        offset,
    )
}

/// [`definition_at`] over already-lowered statements.
pub fn definition_at_lowered(
    output: &AnalysisOutput,
    schema: &SchemaIndex,
    source: &SourceId,
    statements: &[ast::Spanned<ast::Statement>],
    offset: u32,
) -> Option<DefinitionTarget> {
    let mut best: Option<(u32, SourceSpan)> = None;
    let mut consider = |cover: ByteRange, target: SourceSpan| {
        if offset < cover.start() || offset > cover.end() {
            return;
        }
        let width = cover.end().saturating_sub(cover.start());
        let is_smaller = best
            .as_ref()
            .is_none_or(|(best_width, _)| width < *best_width);
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

    // Table, field, and `fn::` references resolved by walking the lowered
    // statement tree — the analyzed statements carry no per-reference spans, so
    // (as hover does) the identifier under the cursor is located here and
    // resolved against the schema. Only definitely-known definitions produce a
    // target; anything uncertain is skipped (low-FP).
    let mut collector = SchemaDefs {
        schema,
        source,
        offset,
        bindings: &bindings,
        param_spans: std::collections::HashMap::new(),
        root: None,
        out: Vec::new(),
    };
    for statement in statements {
        collector.visit_statement(statement);
    }
    for (cover, target) in collector.out {
        consider(cover, target);
    }

    best.map(|(_, span)| DefinitionTarget { span })
}

/// Markdown for a variable/parameter/field symbol: an optional bold caption
/// line (already-formatted markdown, e.g. ``local `$direct` ``) above a fenced
/// `surql` type line the editor syntax-highlights. Records and literal objects
/// additionally get their field shape as an indented `surql` block.
///
/// `ctx` is what the popover is answering. A definition site mirrors the
/// binding as written ([`KindContext::Declared`]); an *occurrence* reports the
/// kind in force at the cursor and names the members that are live there —
/// "what can this be here" is a different question from "what was this
/// declared as", and `option<string>` answers the second one.
fn symbol_markdown(
    caption: Option<&str>,
    name: &str,
    kind: Option<&Kind>,
    ctx: KindContext<'_>,
    schema: &SchemaIndex,
) -> String {
    let rendered = kind.map(|kind| render(kind, ctx));
    let text = rendered
        .as_ref()
        .map_or_else(|| "unknown".to_string(), |rendered| rendered.text.clone());
    let mut markdown = String::new();
    if let Some(caption) = caption {
        markdown.push_str(&format!("**{caption}**\n"));
    }
    markdown.push_str(&format!("```surql\n{name}: {text}\n```"));
    // The "why" line, when the analysis proved one. A hover that shows a kind
    // narrower than the declaration and says nothing about why reads as a bug
    // in the tool.
    if let Some(note) = rendered
        .as_ref()
        .and_then(|rendered| rendered.note.as_ref())
    {
        markdown.push_str(&format!("\n\n{note}"));
    }
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
            let kind = field
                .kind
                .as_ref()
                .map_or_else(|| "any".to_string(), render_kind);
            format!("{}: {kind}", field.path.join("."))
        })
        .collect()
}

/// Collects hover candidates for table references and field names by walking
/// a freshly-lowered statement tree. Each candidate is a `(span, markdown)`
/// pair the caller feeds through the smallest-covering `consider` closure.
///
/// Field resolution tracks the table in scope (a SELECT's single `FROM`
/// source, a mutation's target, a `DEFINE FIELD`'s table — delivered by
/// [`Visitor::visit_row_scope`]) and follows `record<>` links: `author.name`
/// on `post` resolves `name` against the linked `user`. Ambiguity (multiple
/// sources, an opaque traversal) drops the scope so nothing misleading is
/// shown.
struct SchemaHovers<'a> {
    schema: &'a SchemaIndex,
    source: &'a SourceId,
    offset: u32,
    /// The inferred kind of each `LET`/`FOR` variable in scope, by name,
    /// for hovering `$var` uses and `$var[i].field` idioms.
    let_kinds: &'a std::collections::HashMap<String, Kind>,
    /// Every guard-narrowed region analysis recorded, so a `$var` (or a
    /// narrowed `$var.field` path) under the cursor resolves to the kind in
    /// force *there* rather than the one at its binding site.
    narrowings: &'a [NarrowingAnalysis],
    /// Declared `DEFINE FUNCTION` parameters in scope while walking a function
    /// body, by name → declared kind. Populated on entry to the body and
    /// restored on exit, so `$param` uses deep in the body hover their declared
    /// type. A `LET` of the same name shadows it (`let_kinds` is checked first).
    param_kinds: std::collections::HashMap<String, Kind>,
    /// The row table the clause being walked evaluates against, as set by
    /// [`Visitor::visit_row_scope`]; `None` outside a statement's row clauses
    /// or when the row source is not a single plain table.
    root: Option<String>,
    out: Vec<(SourceSpan, String)>,
}

impl SchemaHovers<'_> {
    fn covers(&self, span: ByteRange) -> bool {
        self.offset >= span.start() && self.offset <= span.end()
    }

    /// The kind of a `$var` **at the cursor**: a guard narrowing in force here
    /// wins over the binding's own kind, and a `LET`/`FOR` binding wins over a
    /// function parameter of the same name.
    ///
    /// The cursor is the right position to ask about because every hover this
    /// walker emits is for a span covering it — so "the kind at `self.offset`"
    /// is exactly "the kind at the occurrence being hovered".
    fn var_kind(&self, name: &str) -> Option<Kind> {
        self.narrowed_kind(name).or_else(|| {
            self.let_kinds
                .get(name)
                .or_else(|| self.param_kinds.get(name))
                .cloned()
        })
    }

    /// The flow-narrowed kind for `path` (a bare name, or a
    /// `param.field.field` key) at the cursor, if a guard proved one there.
    fn narrowed_kind(&self, path: &str) -> Option<Kind> {
        narrowed_kind_at(self.narrowings, self.source, path, self.offset).cloned()
    }

    /// The claim that proved the narrowing in force for `path` here, when the
    /// analysis recorded one. Paired with [`Self::narrowed_kind`], so a hover
    /// never shows a tightened kind without the reason beside it.
    fn narrowed_by(&self, path: &str) -> Option<&str> {
        narrowing_at(self.narrowings, self.source, path, self.offset)
            .and_then(|narrowing| narrowing.by.as_deref())
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
                symbol_markdown(
                    Some(&self.var_caption(name)),
                    &format!("${name}"),
                    Some(&kind),
                    KindContext::Occurrence {
                        proved: self.narrowed_by(name),
                    },
                    self.schema,
                ),
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

    /// The hover for a field segment resolved against a schema table, when the
    /// cursor is on it and the field has a declared kind.
    fn table_field_hover(&mut self, field: &ResolvedField<'_>, name: &str, span: ByteRange) {
        if !self.covers(span) {
            return;
        }
        if let Some(kind) = &field.kind {
            let source_span = SourceSpan::new(self.source.clone(), span);
            self.out.push((
                source_span,
                symbol_markdown(
                    Some(&format!("field `{name}` on `{}`", field.def.name)),
                    name,
                    Some(kind),
                    KindContext::Declared,
                    self.schema,
                ),
            ));
        }
    }

    /// Resolves an idiom rooted in a known-kind `$var` (`$direct[0].role`):
    /// steps a value kind through subscripts (into the array/set element) and
    /// fields (into a literal object's entry, or across a single `record<>`
    /// link into schema-resolved fields), emitting a hover for the `$var`
    /// token and each resolvable segment.
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
                symbol_markdown(
                    Some(&self.var_caption(name)),
                    &format!("${name}"),
                    Some(&root_kind),
                    KindContext::Occurrence {
                        proved: self.narrowed_by(name),
                    },
                    self.schema,
                ),
            ));
        }
        // `current` is a value kind; once traversal crosses a `record<>` link
        // it switches to table mode and `scope` — the same schema-based
        // resolver the row-table walker uses — takes over.
        let mut current = Some(root_kind);
        let mut scope = FieldScope::new(None);
        // The plain `param.field.field` prefix walked so far — the key a field
        // guard (`IF $x.parent = NONE …`) narrows under. Anything that is not a
        // plain field step (a subscript, a link crossing) ends the key, since
        // only the exact written path narrows.
        let mut value_path: Vec<String> = Vec::new();
        for part in idiom.parts.iter().skip(1) {
            match &part.node {
                IdiomPart::Index(inner) => {
                    self.visit_expr(inner);
                    current = current.as_ref().and_then(collection_element_kind);
                    scope.reset();
                    value_path.clear();
                }
                IdiomPart::All | IdiomPart::Last => {
                    current = current.as_ref().and_then(collection_element_kind);
                    scope.reset();
                    value_path.clear();
                }
                IdiomPart::Field(field) => {
                    // A value kind that is a single `record<>` link enters its
                    // schema table before this field resolves.
                    if scope.table.is_none() {
                        if let Some(linked) = current.as_ref().and_then(record_link_target) {
                            scope.enter(linked);
                            current = None;
                        }
                    }
                    // Schema-table mode: resolve the field against the table.
                    if scope.table.is_some() {
                        if let Some(resolved) = scope.step(self.schema, field) {
                            self.table_field_hover(&resolved, field, part.span);
                        }
                        continue;
                    }
                    // Object-value mode: index into a literal object's entry.
                    let Some(kind) = current.clone() else {
                        return;
                    };
                    value_path.push(field.to_string());
                    // A guard on this exact path (`$x.parent != NONE`) refines
                    // it downstream, exactly as one on the bare `$x` does.
                    let next = self
                        .narrowed_kind(&format!("{name}.{}", value_path.join(".")))
                        .or_else(|| field_kind(&kind, field, self.schema));
                    if self.covers(part.span) {
                        if let Some(next) = &next {
                            let span = SourceSpan::new(self.source.clone(), part.span);
                            self.out.push((
                                span,
                                symbol_markdown(
                                    Some(&format!("field `{field}`")),
                                    field,
                                    Some(next),
                                    KindContext::Occurrence { proved: None },
                                    self.schema,
                                ),
                            ));
                        }
                    }
                    current = next;
                }
                // Anything else (methods, graph steps, filters, destructuring,
                // recursion, the chaining markers) is opaque here.
                IdiomPart::Start(_)
                | IdiomPart::Graph { .. }
                | IdiomPart::Destructure(_)
                | IdiomPart::Where(_)
                | IdiomPart::Method { .. }
                | IdiomPart::Recurse { .. }
                | IdiomPart::Optional
                | IdiomPart::Flatten
                | IdiomPart::Partial(_) => return,
            }
        }
    }
}

impl Visitor for SchemaHovers<'_> {
    fn visit_row_scope(&mut self, table: Option<&str>, walk: impl FnOnce(&mut Self)) {
        let saved = std::mem::replace(&mut self.root, table.map(str::to_string));
        walk(self);
        self.root = saved;
    }

    /// A table reference (`FROM person`, `ON person`, a graph edge): show the
    /// `table <name>` popover, but only for a table the schema actually knows.
    fn visit_table_ref(&mut self, name: &ast::Spanned<String>) {
        if !self.covers(name.span) {
            return;
        }
        if let Some(def) = self.schema.table(&name.node) {
            let span = SourceSpan::new(self.source.clone(), name.span);
            self.out.push((span, table_markdown(def, self.schema)));
        }
    }

    fn visit_expr(&mut self, expr: &ast::Spanned<ast::Expr>) {
        if let ast::Expr::Param(name) = &expr.node {
            self.param_use(expr.span, name);
        }
        visit::walk_expr(self, expr);
    }

    fn visit_call(&mut self, call: &ast::Call) {
        self.call_signature(call);
        visit::walk_call(self, call);
    }

    /// A DEFINE FUNCTION body is ordinary statement territory: its `LET`/`FOR`
    /// var uses, `fn::` calls, and idioms all resolve the same as at top level,
    /// so walk it — but first bring the declared parameters into scope so
    /// `$param` uses in the body (and the signature tokens themselves) hover
    /// their declared type.
    fn visit_define_function(&mut self, func: &ast::DefineFunction) {
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
                            KindContext::Declared,
                            self.schema,
                        ),
                    ));
                }
                self.param_kinds.insert(name.node.clone(), kind);
            }
        }
        visit::walk_define_function(self, func);
        self.param_kinds = saved;
    }

    /// An idiom rooted in a known-kind `$var` (`$direct[0].role`) resolves
    /// through value kinds; every other idiom resolves its field parts against
    /// the row table in scope, re-rooting on `record<>` links so linked-table
    /// fields type too.
    fn visit_idiom(&mut self, idiom: &ast::Idiom) {
        if let Some(ast::Spanned {
            node: ast::IdiomPart::Start(inner),
            ..
        }) = idiom.parts.first()
        {
            if let ast::Expr::Param(name) = &inner.node {
                if self.var_kind(name).is_some() {
                    self.resolve_value_idiom(name, inner.span, idiom);
                    return;
                }
            }
        }
        let schema = self.schema;
        let root = self.root.clone();
        walk_field_idiom(
            self,
            schema,
            root.as_deref(),
            idiom,
            |this, field, name, span| {
                this.table_field_hover(field, name, span);
            },
        );
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
    /// The row table in scope, as set by [`Visitor::visit_row_scope`].
    root: Option<String>,
    out: Vec<(ByteRange, SourceSpan)>,
}

impl SchemaDefs<'_> {
    fn covers(&self, span: ByteRange) -> bool {
        self.offset >= span.start() && self.offset <= span.end()
    }
}

impl Visitor for SchemaDefs<'_> {
    fn visit_row_scope(&mut self, table: Option<&str>, walk: impl FnOnce(&mut Self)) {
        let saved = std::mem::replace(&mut self.root, table.map(str::to_string));
        walk(self);
        self.root = saved;
    }

    /// A table reference → its `DEFINE TABLE` name span, for tables the schema
    /// knows.
    fn visit_table_ref(&mut self, name: &ast::Spanned<String>) {
        if !self.covers(name.span) {
            return;
        }
        if let Some(def) = self.schema.table(&name.node) {
            self.out.push((name.span, def.name_span.clone()));
        }
    }

    /// A `$param` / `LET` variable use → its binding site. A `LET`/`DEFINE
    /// PARAM` binding wins over a function parameter of the same name.
    fn visit_expr(&mut self, expr: &ast::Spanned<ast::Expr>) {
        if let ast::Expr::Param(name) = &expr.node {
            if self.covers(expr.span) {
                if let Some(target) = self
                    .bindings
                    .get(name)
                    .or_else(|| self.param_spans.get(name))
                {
                    self.out.push((expr.span, target.clone()));
                }
            }
        }
        visit::walk_expr(self, expr);
    }

    /// A `fn::` call → its `DEFINE FUNCTION` name span.
    fn visit_call(&mut self, call: &ast::Call) {
        if self.covers(call.path.span) {
            if let Some(func) = self.schema.function(&call.path.node) {
                self.out.push((call.path.span, func.name_span.clone()));
            }
        }
        visit::walk_call(self, call);
    }

    /// A DEFINE FUNCTION body is ordinary statement territory — walk it with
    /// the declared parameters in scope, so go-to-def on a `$param` use in the
    /// body jumps to its declaration in the signature.
    fn visit_define_function(&mut self, func: &ast::DefineFunction) {
        let saved = std::mem::take(&mut self.param_spans);
        for (name, _ty) in &func.params {
            self.param_spans.insert(
                name.node.clone(),
                SourceSpan::new(self.source.clone(), name.span),
            );
        }
        visit::walk_define_function(self, func);
        self.param_spans = saved;
    }

    /// Resolves an idiom's field parts against the table in scope (re-rooting
    /// on `record<>` links exactly as hover does) and emits the owning `DEFINE
    /// FIELD`'s name span. Only a directly-declared field emits; object
    /// prefixes, implicit `id`/`in`/`out`, and schemaless fields have no
    /// declaration to jump to.
    fn visit_idiom(&mut self, idiom: &ast::Idiom) {
        let schema = self.schema;
        let root = self.root.clone();
        walk_field_idiom(
            self,
            schema,
            root.as_deref(),
            idiom,
            |this, field, _name, span| {
                if !this.covers(span) {
                    return;
                }
                if let Some(decl) = field.def.fields.get(&field.path.join(".")) {
                    this.out.push((span, decl.name_span.clone()));
                }
            },
        );
    }
}

/// Where a field path currently resolves: the schema table in scope and the
/// object-field prefix walked within it (`profile` in `profile.email`).
/// Shared by the hover and definition walkers so the two agree on which
/// table every segment names.
struct FieldScope {
    /// The table the next field segment resolves against; `None` once the
    /// shape has gone opaque (a leading value, a graph step, an unknown table).
    table: Option<String>,
    /// Field segments accumulated relative to `table` — a nested object path.
    segments: Vec<String>,
}

/// One field segment resolved by [`FieldScope::step`].
struct ResolvedField<'s> {
    /// The table the segment was resolved against.
    def: &'s crate::schema::TableDef,
    /// The full dotted path within that table, this segment included.
    path: Vec<String>,
    /// The field's declared kind, when the schema has one for the path.
    kind: Option<Kind>,
}

impl FieldScope {
    fn new(root: Option<&str>) -> Self {
        Self {
            table: root.map(str::to_string),
            segments: Vec::new(),
        }
    }

    /// The shape is opaque from here on: nothing further resolves.
    fn reset(&mut self) {
        self.table = None;
        self.segments.clear();
    }

    /// Re-roots on `table` (a `record<>` link just crossed).
    fn enter(&mut self, table: String) {
        self.table = Some(table);
        self.segments.clear();
    }

    /// Resolves `field` against the table in scope, then advances the scope:
    /// into the linked table on a `record<>` link, deeper into the same table
    /// on a nested object, or out of scope when the shape goes opaque. `None`
    /// when no known table is in scope (which also ends resolution).
    fn step<'s>(&mut self, schema: &'s SchemaIndex, field: &str) -> Option<ResolvedField<'s>> {
        let table = self.table.clone()?;
        let Some(def) = schema.table(&table) else {
            self.table = None;
            return None;
        };
        let mut path = self.segments.clone();
        path.push(field.to_string());
        let kind = crate::analyzer::data::select::resolve_field_path(schema, def, &path);
        match kind.as_ref().and_then(record_link_target) {
            Some(linked) => self.enter(linked),
            None if kind.is_some() => self.segments.clone_from(&path),
            None => self.table = None,
        }
        Some(ResolvedField { def, path, kind })
    }
}

/// Walks an idiom's parts for a schema-resolving visitor: every plain field
/// segment a known table is in scope for is handed to `on_field` (with the
/// resolution and the segment's span), sub-expressions are visited through
/// `visitor`, a leading value or graph step makes the rest opaque, and a
/// destructure's sub-paths resolve in the scope reached so far.
fn walk_field_idiom<V: Visitor>(
    visitor: &mut V,
    schema: &SchemaIndex,
    root: Option<&str>,
    idiom: &ast::Idiom,
    mut on_field: impl FnMut(&mut V, &ResolvedField<'_>, &str, ByteRange),
) {
    use ast::IdiomPart;
    let mut scope = FieldScope::new(root);
    for part in &idiom.parts {
        match &part.node {
            IdiomPart::Start(inner) => {
                visitor.visit_expr(inner);
                // Rooted in a leading value, not the row table.
                scope.reset();
            }
            IdiomPart::Field(name) => {
                if let Some(field) = scope.step(schema, name) {
                    on_field(visitor, &field, name, part.span);
                }
            }
            IdiomPart::Index(inner) | IdiomPart::Where(inner) => visitor.visit_expr(inner),
            IdiomPart::Method { name: _, args } => {
                for arg in args {
                    visitor.visit_expr(arg);
                }
            }
            IdiomPart::Graph { dir: _, step } => {
                visitor.visit_graph_step(step);
                // The shape after a graph step is opaque here.
                scope.reset();
            }
            IdiomPart::Destructure(idioms) => {
                visitor.visit_row_scope(scope.table.as_deref(), |visitor| {
                    for sub in idioms {
                        visitor.visit_idiom(&sub.node);
                    }
                });
            }
            IdiomPart::All
            | IdiomPart::Last
            | IdiomPart::Recurse { .. }
            | IdiomPart::Optional
            | IdiomPart::Flatten => {}
            IdiomPart::Partial(partial) => visitor.visit_partial(partial),
        }
    }
}

/// The kind of `field` within a value kind, for hover: [`crate::kinds::project`]
/// with the schema in hand, so a literal object is read by key, an
/// `option<{…}>` keeps its optionality, and a link the table-scope walk did
/// not enter (a multi-table `record<a | b>`) still resolves through the schema.
fn field_kind(kind: &Kind, field: &str, schema: &SchemaIndex) -> Option<Kind> {
    crate::kinds::project(
        kind,
        &crate::schema::FieldStep::Field(field.to_string()),
        Some(schema),
    )
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
    use surrealdb_types::KindLiteral;

    fn analyze(text: &str) -> (AnalysisOutput, SchemaIndex, SourceId) {
        let mut workspace = Workspace::default();
        let source = workspace.add_virtual_source("query".into(), text.into());
        let output = analyze_workspace(&workspace);
        let analysis = output.sources[&source].clone();
        (analysis, output.schema, source)
    }

    #[test]
    fn function_return_hints_ghost_untyped_bodies_only() {
        let text = "DEFINE FUNCTION fn::double($x: int) { RETURN $x * 2; };\n\
                    DEFINE FUNCTION fn::noop($x: int) { LET $y = $x; };\n\
                    DEFINE FUNCTION fn::greet() -> string { RETURN 'hi'; };";
        let (_output, schema, source) = analyze(text);
        let labels: Vec<String> = function_return_hints(text, &source, &schema)
            .into_iter()
            .map(|hint| hint.label)
            .collect();
        // `double`'s inferred `int` and `noop`'s unit body get ghosts, in source
        // order; the explicitly-typed `greet` gets none (its return is written).
        assert_eq!(labels, vec!["-> <int>".to_string(), "-> none".to_string()]);
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

    /// A schema whose `ONLY` select yields an `option<{…}>` — the shape every
    /// narrowing-hover test below guards on.
    const NARROWING_SCHEMA: &str = "DEFINE TABLE unit SCHEMAFULL;\n\
                                    DEFINE FIELD label ON unit TYPE string;\n\
                                    DEFINE FIELD parent ON unit TYPE option<record<unit>>;\n";

    /// The kind hover reports for the `n`th `$x` occurrence in `text`.
    fn hover_kind_at_occurrence(text: &str, n: usize) -> String {
        let (output, schema, source) = analyze(text);
        let mut from = 0;
        for _ in 0..n {
            from = text[from..].find("$x").expect("occurrence exists") + from + 2;
        }
        let hover = hover_at(&output, &schema, &source, text, from as u32 - 1)
            .unwrap_or_else(|| panic!("hover over occurrence {n}"));
        hover.markdown
    }

    #[test]
    fn hover_keeps_the_declared_kind_at_and_before_a_guard() {
        // NEW-11: hover is per occurrence. At the binding and at the guard that
        // tests it, the binding is still what it was declared/inferred to be —
        // narrowing must not reach backwards. The binding site mirrors the
        // binding as written (`option<…>`); the occurrence at the guard names
        // the `none` that is still live there.
        let text = format!(
            "{NARROWING_SCHEMA}\
             LET $x = (SELECT label FROM ONLY unit LIMIT 1);\n\
             IF $x = NONE THEN THROW 'missing' END;\n\
             RETURN $x.label;\n"
        );

        assert!(
            hover_kind_at_occurrence(&text, 1).contains("option<"),
            "binding site: {}",
            hover_kind_at_occurrence(&text, 1)
        );
        assert!(
            hover_kind_at_occurrence(&text, 2).contains("none |"),
            "at the guard: {}",
            hover_kind_at_occurrence(&text, 2)
        );
    }

    #[test]
    fn hover_reports_the_narrowed_kind_after_a_diverging_guard() {
        let text = format!(
            "{NARROWING_SCHEMA}\
             LET $x = (SELECT label FROM ONLY unit LIMIT 1);\n\
             IF $x = NONE THEN THROW 'missing' END;\n\
             RETURN $x.label;\n"
        );

        let after = hover_kind_at_occurrence(&text, 3);
        assert!(
            !after.contains("none") && after.contains("label"),
            "past the guard the binding cannot be NONE: {after}"
        );
    }

    /// A hover that shows a kind narrower than the declaration and says
    /// nothing about why reads as a bug in the tool. The claim is available as
    /// a value for the first time, so it is shown.
    #[test]
    fn hover_says_why_the_kind_is_narrower_than_the_declaration() {
        let text = format!(
            "{NARROWING_SCHEMA}\
             LET $x = (SELECT label FROM ONLY unit LIMIT 1);\n\
             IF $x = NONE THEN THROW 'missing' END;\n\
             RETURN $x.label;\n"
        );

        let at_guard = hover_kind_at_occurrence(&text, 2);
        assert!(
            !at_guard.contains("narrowed by"),
            "at the guard nothing is narrowed yet: {at_guard}"
        );
        let after = hover_kind_at_occurrence(&text, 3);
        assert!(
            after.contains("narrowed by `$x != NONE`"),
            "past the guard the hover must say what proved it: {after}"
        );
    }

    #[test]
    fn hover_narrowing_stops_with_the_scope_that_established_it() {
        // A guard inside a block narrows the rest of *that block*. The
        // statements after the block see the binding again as it was, so a
        // region that leaked would show up right here.
        let text = format!(
            "{NARROWING_SCHEMA}\
             LET $x = (SELECT label FROM ONLY unit LIMIT 1);\n\
             LET $inner = {{ IF $x = NONE THEN THROW 'missing' END; RETURN $x.label; }};\n\
             RETURN $x;\n"
        );

        assert!(
            !hover_kind_at_occurrence(&text, 3).contains("none"),
            "inside the block, past the guard: {}",
            hover_kind_at_occurrence(&text, 3)
        );
        assert!(
            hover_kind_at_occurrence(&text, 4).contains("none |"),
            "after the block the guard proves nothing: {}",
            hover_kind_at_occurrence(&text, 4)
        );
    }

    #[test]
    fn hover_narrows_a_guarded_field_path_only_past_its_guard() {
        // The same rule one level down: `$x.parent` is refined by a guard on
        // that exact path, and only after it.
        let text = format!(
            "{NARROWING_SCHEMA}\
             LET $x = (SELECT label, parent FROM ONLY unit LIMIT 1);\n\
             IF $x = NONE THEN THROW 'missing' END;\n\
             IF $x.parent = NONE THEN THROW 'root' END;\n\
             RETURN $x.parent;\n"
        );
        let (output, schema, source) = analyze(&text);
        let at_guard = text.rfind("$x.parent = NONE").expect("guard present") + 4;
        let past_guard = text.rfind("$x.parent").expect("use present") + 4;

        let guard_hover = hover_at(&output, &schema, &source, &text, at_guard as u32)
            .expect("hover on the guarded path");
        let past_hover = hover_at(&output, &schema, &source, &text, past_guard as u32)
            .expect("hover past the guard");
        assert!(
            guard_hover.markdown.contains("parent: none | record<unit>"),
            "at its own guard the path can still be NONE: {}",
            guard_hover.markdown
        );
        assert!(
            past_hover.markdown.contains("parent: record<unit>"),
            "past the guard the path is a bare link: {}",
            past_hover.markdown
        );
    }

    #[test]
    fn analysis_records_one_narrowed_region_per_guard() {
        // The data hover reads is part of `AnalysisOutput`, so inlay hints (and
        // any other cache-only editor feature) can answer per occurrence from
        // exactly the same facts — without re-running analysis.
        let text = format!(
            "{NARROWING_SCHEMA}\
             LET $x = (SELECT label FROM ONLY unit LIMIT 1);\n\
             IF $x = NONE THEN THROW 'missing' END;\n\
             RETURN $x.label;\n"
        );
        let (output, _schema, _source) = analyze(&text);

        // Two regions, one per side of the guard. Inside the THEN body the
        // condition holds, so the binding is NONE there…
        let body = text.find("THROW").expect("body present");
        let in_body = output
            .narrowings
            .iter()
            .find(|narrowing| {
                narrowing.path == "x" && narrowing.span.range().start() as usize == body
            })
            .expect("the THEN body is a narrowed region");
        assert_eq!(in_body.kind, Kind::None);

        // …and past the guard statement it is the complement, over a region
        // that opens where the guard ends and runs to the end of the top-level
        // statement sequence.
        let guard_end = text.find("END").expect("guard present") + "END".len();
        let last_statement = text.find("RETURN $x.label").expect("use present");
        let past_guard = output
            .narrowings
            .iter()
            .find(|narrowing| {
                narrowing.path == "x"
                    && (guard_end..=last_statement)
                        .contains(&(narrowing.span.range().start() as usize))
            })
            .expect("the statements past the guard are a narrowed region");
        assert!(
            !matches!(past_guard.kind, Kind::Either(_)),
            "{:?}",
            past_guard.kind
        );
        assert_eq!(past_guard.span.range().end() as usize, text.len());
    }

    #[test]
    fn hover_resolves_a_param_use_kind() {
        // `$id` is compared against the int field `age`, so it constrains
        // to int.
        let text = "DEFINE TABLE person;\nDEFINE FIELD age ON person TYPE int;\nSELECT * FROM person WHERE age = $id;";
        let (output, schema, source) = analyze(text);
        let offset = text.find("$id").expect("param present") as u32 + 1;
        let hover = hover_at(&output, &schema, &source, text, offset).expect("hover over $id");

        assert!(
            hover.markdown.contains("$id: int"),
            "got: {}",
            hover.markdown
        );
    }

    #[test]
    fn hover_over_a_table_name_lists_its_fields() {
        let text = "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;\nDEFINE FIELD age ON person TYPE int;";
        let (output, schema, source) = analyze(text);
        // Cursor on the `person` name in the DEFINE TABLE statement.
        let offset = text.find("person").expect("table name present") as u32 + 1;
        let hover = hover_at(&output, &schema, &source, text, offset).expect("hover over table");

        assert!(
            hover.markdown.contains("DEFINE TABLE person"),
            "got: {}",
            hover.markdown
        );
        assert!(
            hover.markdown.contains("name: string"),
            "got: {}",
            hover.markdown
        );
        assert!(
            hover.markdown.contains("age: int"),
            "got: {}",
            hover.markdown
        );
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

        assert!(
            hover.markdown.contains("$value: string"),
            "got: {}",
            hover.markdown
        );
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
            event_hover
                .markdown
                .contains("'CREATE' | 'UPDATE' | 'DELETE'"),
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
        let hover =
            hover_at(&output, &schema, &source, text, offset).expect("hover over FROM table");

        assert!(
            hover.markdown.contains("DEFINE TABLE person"),
            "got: {}",
            hover.markdown
        );
        assert!(
            hover.markdown.contains("name: string"),
            "got: {}",
            hover.markdown
        );
        assert!(
            hover.markdown.contains("age: int"),
            "got: {}",
            hover.markdown
        );
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

        assert!(
            hover.markdown.contains("age: int"),
            "got: {}",
            hover.markdown
        );
    }

    #[test]
    fn subscript_element_kind_unions_every_collection_arm() {
        // NEW-5: `array<int> | array<string>` subscripts to `int | string`.
        // Taking only the first arm reported a confidently *wrong* `int`.
        let mixed = Kind::Either(vec![
            Kind::Array(Box::new(Kind::Int), None),
            Kind::Array(Box::new(Kind::String), None),
        ]);
        assert_eq!(
            collection_element_kind(&mixed),
            Some(Kind::Either(vec![Kind::Int, Kind::String]))
        );

        // `option<array<T>>` subscripts to `option<T>`: 3.2.3 evaluates
        // `NONE[0]` to `NONE` rather than failing, so the `NONE` arm carries
        // through the index instead of contributing nothing.
        let optional = Kind::Either(vec![Kind::None, Kind::Array(Box::new(Kind::Int), None)]);
        assert_eq!(
            collection_element_kind(&optional),
            Some(Kind::Either(vec![Kind::None, Kind::Int]))
        );

        // A set arm participates in the union just like an array arm.
        let array_or_set = Kind::Either(vec![
            Kind::Array(Box::new(Kind::Int), None),
            Kind::Set(Box::new(Kind::String), None),
        ]);
        assert_eq!(
            collection_element_kind(&array_or_set),
            Some(Kind::Either(vec![Kind::Int, Kind::String]))
        );

        // No collection arm at all: still no element kind (stay silent).
        assert_eq!(
            collection_element_kind(&Kind::Either(vec![Kind::None, Kind::Int])),
            None
        );
        assert_eq!(collection_element_kind(&Kind::String), None);
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
        let author_hover = hover_at(&output, &schema, &source, text, author_offset)
            .expect("hover over link field");
        assert!(
            author_hover.markdown.contains("author: record<user>"),
            "got: {}",
            author_hover.markdown
        );

        // The field reached through the link resolves against the linked table.
        let name_offset = text.rfind("name").expect("linked field present") as u32 + 1;
        let name_hover = hover_at(&output, &schema, &source, text, name_offset)
            .expect("hover over linked field");
        assert!(
            name_hover.markdown.contains("name: string"),
            "got: {}",
            name_hover.markdown
        );
    }

    #[test]
    fn hover_crosses_a_union_record_link_to_the_linked_field() {
        // A union link `record<user | admin>` can't be re-rooted one segment at
        // a time (two targets), so the field past it must resolve through the
        // link-crossing resolver. Both variants agree on `name: string`.
        let text = "DEFINE TABLE user SCHEMAFULL;\n\
             DEFINE FIELD name ON user TYPE string;\n\
             DEFINE TABLE admin SCHEMAFULL;\n\
             DEFINE FIELD name ON admin TYPE string;\n\
             DEFINE TABLE post SCHEMAFULL;\n\
             DEFINE FIELD author ON post TYPE record<user | admin>;\n\
             SELECT author.name FROM post;";
        let (output, schema, source) = analyze(text);

        let name_offset = text.rfind("name").expect("linked field present") as u32 + 1;
        let name_hover = hover_at(&output, &schema, &source, text, name_offset)
            .expect("hover over union-linked field");
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
        let expected = text
            .find("name ON person")
            .expect("DEFINE FIELD name present") as u32;
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

        let expected = text
            .find("name ON user")
            .expect("DEFINE FIELD name present") as u32;
        assert_eq!(target.span.range().start(), expected);
    }

    #[test]
    fn definition_of_a_function_call_points_at_its_define_function() {
        let text = "DEFINE FUNCTION fn::greet($who: string) -> string { RETURN $who; };\n\
             RETURN fn::greet('ada');";
        let (output, schema, source) = analyze(text);
        let offset = text.rfind("fn::greet").expect("call present") as u32 + 2;
        let target =
            definition_at(&output, &schema, &source, text, offset).expect("definition of function");

        let expected = text
            .find("fn::greet")
            .expect("DEFINE FUNCTION name present") as u32;
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
        let field_target = definition_at(
            &analysis,
            &output.schema,
            &query_source,
            query_text,
            field_offset,
        )
        .expect("cross-file field definition");
        assert_eq!(field_target.span.source(), &schema_source);

        // Table reference → DEFINE TABLE in the schema source.
        let table_offset = query_text.find("person").expect("table present") as u32 + 1;
        let table_target = definition_at(
            &analysis,
            &output.schema,
            &query_source,
            query_text,
            table_offset,
        )
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
            assert!(
                !hover.markdown.contains("record<"),
                "got: {}",
                hover.markdown
            );
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
        assert!(
            bind.markdown.contains("$direct: array<"),
            "got: {}",
            bind.markdown
        );
        assert!(
            bind.markdown.contains("role: string"),
            "got: {}",
            bind.markdown
        );

        // Use site: `$direct[0].role` — the `$direct` token resolves to the
        // same array kind.
        let use_offset = BODY_FIXTURE.rfind("$direct").expect("use present") as u32 + 1;
        let used = hover_at(&output, &schema, &source, BODY_FIXTURE, use_offset)
            .expect("hover over body LET use");
        assert!(
            used.markdown.contains("$direct: array<"),
            "got: {}",
            used.markdown
        );
    }

    #[test]
    fn hover_over_a_subscript_idiom_resolves_the_element_field() {
        let (output, schema, source) = analyze(BODY_FIXTURE);
        // `$direct[0].role`: index into the array element `{ role, unit }`,
        // then `.role` is `string`.
        let role_use = BODY_FIXTURE.rfind(".role").expect("subscript present") as u32 + 1;
        let hover = hover_at(&output, &schema, &source, BODY_FIXTURE, role_use)
            .expect("hover over subscript field");
        assert!(
            hover.markdown.contains("role: string"),
            "got: {}",
            hover.markdown
        );
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
            hover
                .markdown
                .contains("fn::org::unit($organization: record<unit>) -> record<unit>"),
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

    /// Parses `label` back as a SurrealQL type, the way an editor's reader
    /// would have to. `None` when it is not one.
    fn parse_as_type(label: &str) -> Option<Kind> {
        let text = format!("DEFINE FIELD f ON t TYPE {label};");
        let parsed = parse_source(SourceId::new("label"), text.as_str()).ok()?;
        let statements = surrealql_analyzer_syntax::lower::lower_statements(&parsed);
        let ast::Statement::Define(ast::DefineStmt::Field(field)) = &statements.first()?.node
        else {
            return None;
        };
        let parsed_kind = crate::schema::kind_from_type_expr(&field.ty.as_ref()?.node, &text);
        parsed_kind.partial.is_empty().then_some(parsed_kind.kind)?
    }

    #[test]
    fn inlay_label_elides_an_over_long_object_kind() {
        // An over-long kind is elided *structurally*, so what reaches the
        // editor is still a type: a character cut produced
        // `{ field_number_0: string, field_number_1: string, fi…`, which no
        // reader can parse and which stops mid-name.
        let wide = Kind::Literal(KindLiteral::Object(
            (0..12)
                .map(|i| (format!("field_number_{i}"), Kind::String))
                .collect(),
        ));
        assert!(render_kind(&wide).chars().count() > INLAY_LABEL_MAX);

        let label = glance_label(&wide);
        assert!(
            label.chars().count() <= INLAY_LABEL_MAX,
            "over budget: {label}"
        );
        assert!(
            !label.contains('…'),
            "the label is elided by kind, not by character: {label}"
        );

        // The label parses as a type, and every value the binding can hold
        // still satisfies it — the elision widens, so the shorter label is
        // never a claim the value can violate.
        let reparsed = parse_as_type(&label).unwrap_or_else(|| panic!("`{label}` is not a type"));
        assert!(
            crate::kinds::kind_is_assignable_to(&wide, &reparsed),
            "`{label}` must still admit the kind it labels"
        );
    }

    #[test]
    fn an_inlay_label_stays_a_type_for_every_shape_that_overruns() {
        // The budget is spent on the kind, so each of these comes back a
        // shorter *true* type rather than a prefix of a string.
        let long_union = Kind::either(vec![
            Kind::None,
            Kind::String,
            Kind::Int,
            Kind::Datetime,
            Kind::Uuid,
            Kind::Decimal,
            Kind::Duration,
        ]);
        let long_record = Kind::Record(
            (0..8)
                .map(|i| surrealdb_types::Table::from(format!("quite_long_table_{i}")))
                .collect(),
        );
        let nested = Kind::Array(
            Box::new(Kind::Literal(KindLiteral::Object(
                (0..6)
                    .map(|i| (format!("field_number_{i}"), Kind::String))
                    .collect(),
            ))),
            None,
        );
        for kind in [long_union, long_record, nested] {
            let label = glance_label(&kind);
            assert!(
                label.chars().count() <= INLAY_LABEL_MAX,
                "over budget: {label}"
            );
            let reparsed =
                parse_as_type(&label).unwrap_or_else(|| panic!("`{label}` is not a type"));
            assert!(
                crate::kinds::kind_is_assignable_to(&kind, &reparsed),
                "`{label}` must still admit {}",
                render_kind(&kind)
            );
        }
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
        assert!(
            hover.markdown.contains("```surql"),
            "got: {}",
            hover.markdown
        );
        assert!(
            hover
                .markdown
                .contains("DEFINE TABLE has_subsidiary SCHEMAFULL;"),
            "got: {}",
            hover.markdown
        );
        assert!(
            hover
                .markdown
                .contains("TYPE RELATION IN organization OUT organization"),
            "got: {}",
            hover.markdown
        );
        assert!(
            hover.markdown.contains("public: bool"),
            "got: {}",
            hover.markdown
        );
        assert!(
            hover.markdown.contains("status: string"),
            "got: {}",
            hover.markdown
        );
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
            hover
                .markdown
                .contains("$organization: record<organization>"),
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
        assert!(
            hover.markdown.contains("$organization: int"),
            "got: {}",
            hover.markdown
        );
    }

    #[test]
    fn hover_resolves_an_implicit_relation_out_field() {
        let (output, schema, source) = analyze(RELATION_FIXTURE);
        // `out` in `WHERE out = $organization` on the relation table.
        let out_offset = RELATION_FIXTURE.find("WHERE out").expect("out present") as u32 + 6;
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
        let out_offset = RELATION_FIXTURE.find("WHERE out").expect("out present") as u32 + 6;
        assert!(definition_at(&output, &schema, &source, RELATION_FIXTURE, out_offset).is_none());
    }
}
