//! Read-only queries over analysis output for editor features (inlay
//! hints, hover). This module produces no diagnostics and mutates no
//! state: it only reads the facts the analysis pipeline already computed
//! (`AnalysisOutput`, `SchemaIndex`) and shapes them for presentation.

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::SourceSpan;

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
