//! AST-aware hover — uses tree-sitter to find the node at cursor position
//! and resolves contextual hover information.

use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position, Range};

use surrealguard_analyzer::{self as sg, Context, Kind};
use surrealguard_analyzer::types::{display_kind, KindExt, Literal};

use crate::text::{byte_range_to_lsp, position_to_offset};

/// Built-in function signatures for hover display.
struct BuiltinFn {
    name: &'static str,
    signature: &'static str,
    summary: &'static str,
    doc_anchor: &'static str,
    doc_namespace: &'static str,
}

/// Resolve hover for a document at a given position.
///
/// Uses tree-sitter to find the AST node at the cursor, walks up the tree
/// to understand context, and returns appropriate hover content.
pub fn resolve(
    source: &str,
    position: Position,
    ctx: &Context,
) -> Option<Hover> {
    let offset = position_to_offset(source, position);
    let tree = sg::parse(source).ok()?;
    let root = tree.root_node();
    let node = root.descendant_for_byte_range(offset, offset)?;

    let text = node.utf8_text(source.as_bytes()).ok()?.trim().to_string();
    if text.is_empty() {
        return None;
    }

    let node_range = byte_range_to_lsp(source, node.start_byte(), node.end_byte());

    // Walk up the tree to understand context
    let mut current = node;
    loop {
        let Some(parent) = current.parent() else {
            break;
        };

        // Check context based on parent node type
        match parent.kind() {
            // Identifier in FROM clause → table reference
            "from_clause" if current.kind() == "identifier" => {
                return table_hover(&text, ctx, node_range);
            }
            // Identifier in graph path → show relation or target table
            "graph_predicate" | "graph_path" if current.kind() == "identifier" => {
                // Check if this identifier is a valid target for the preceding relation.
                // If the relation doesn't connect to this table, show the relation
                // definition so the user can see why it's wrong.
                if let Some(relation_name) = find_preceding_relation(&current, source) {
                    if let Some(relation_table) = ctx.get_table(&relation_name) {
                        if let surrealguard_analyzer::context::TableKind::Relation { to, .. } = &relation_table.kind {
                            let is_valid_target = to.as_ref()
                                .map(|tables| tables.iter().any(|t| t == &text))
                                .unwrap_or(true);
                            if !is_valid_target {
                                // Invalid target — show the relation so user sees what's allowed
                                return table_hover(&relation_name, ctx, node_range);
                            }
                        }
                    }
                }
                return table_hover(&text, ctx, node_range);
            }
            // Identifier in create_target → table
            "create_target" if current.kind() == "identifier" => {
                return table_hover(&text, ctx, node_range);
            }
            // Identifier in relate_subject → table
            "relate_subject" if current.kind() == "identifier" => {
                return table_hover(&text, ctx, node_range);
            }
            // Field in SET/WHERE/SELECT → field definition
            "field_assignment" if current.kind() == "identifier" => {
                let table = find_statement_table(&parent, source);
                return field_hover(&text, table.as_deref(), ctx, node_range);
            }
            "where_clause" | "select_clause" if current.kind() == "identifier" => {
                let table = find_statement_table(&parent, source);
                return field_hover(&text, table.as_deref(), ctx, node_range);
            }
            // Wildcard → expand fields
            _ if text == "*" => {
                let table = find_statement_table(&current, source);
                return wildcard_hover(table.as_deref(), ctx, node_range);
            }
            // Variable reference
            _ if current.kind() == "variable_name" || current.kind() == "variable" => {
                return variable_hover(&text, ctx, node_range);
            }
            _ => {}
        }

        // DML statement target identifiers (UPDATE user, DELETE user)
        if current.kind() == "identifier" {
            match parent.kind() {
                "update_statement" | "delete_statement" | "upsert_statement" => {
                    // Only if this identifier comes before any clause
                    let is_target = is_target_identifier(&parent, &current);
                    if is_target {
                        return table_hover(&text, ctx, node_range);
                    }
                }
                _ => {}
            }
        }

        current = parent;
    }

    // Fallback: try as table name, then field name
    if ctx.has_table(&text) {
        return table_hover(&text, ctx, node_range);
    }

    // Try as a builtin function
    if let Some(hover) = builtin_function_hover(&text, node_range) {
        return Some(hover);
    }

    None
}

/// Show a table definition with all its fields.
fn table_hover(name: &str, ctx: &Context, range: Range) -> Option<Hover> {
    let table = ctx.get_table(name)?;
    let fields = ctx.get_fields(name);

    let mut header = format!("DEFINE TABLE {}", name);

    // Add TYPE RELATION FROM/TO if applicable
    if let surrealguard_analyzer::context::TableKind::Relation { from, to } = &table.kind {
        header.push_str(" TYPE RELATION");
        if let Some(from_tables) = from {
            header.push_str(&format!(" FROM {}", from_tables.join(" | ")));
        }
        if let Some(to_tables) = to {
            header.push_str(&format!(" TO {}", to_tables.join(" | ")));
        }
    }

    // Add schema mode
    match table.schema_mode {
        surrealguard_analyzer::context::SchemaMode::Schemafull => header.push_str(" SCHEMAFULL"),
        surrealguard_analyzer::context::SchemaMode::Schemaless => header.push_str(" SCHEMALESS"),
    }

    // Show as actual DEFINE statements
    let mut code_lines = vec![format!("{};", header)];
    for field in fields {
        let mut field_def = format!("DEFINE FIELD {} ON {}", field.name, name);
        if let Some(ref typ) = field.typ {
            field_def.push_str(&format!(" TYPE {}", display_kind(typ)));
        }
        if field.readonly {
            field_def.push_str(" READONLY");
        }
        field_def.push(';');
        code_lines.push(format!("  {}", field_def));
    }

    let value = format!("```surql\n{}\n```", code_lines.join("\n"));

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(range),
    })
}

/// Show a field definition.
fn field_hover(name: &str, table: Option<&str>, ctx: &Context, range: Range) -> Option<Hover> {
    // Try specific table first
    if let Some(tbl) = table {
        if let Some(field) = ctx.get_field(tbl, name) {
            let mut def = format!("DEFINE FIELD {} ON {}", name, tbl);
            if let Some(ref typ) = field.typ {
                def.push_str(&format!(" TYPE {}", display_kind(typ)));
            }
            if field.readonly {
                def.push_str(" READONLY");
            }
            if let Some(ref default) = field.default {
                def.push_str(&format!(" {}", default));
            }
            let value = format!("```surql\n{def}\n```");
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value,
                }),
                range: Some(range),
            });
        }
    }

    // Try all tables
    let mut matches = Vec::new();
    for tbl_name in ctx.table_names() {
        if let Some(field) = ctx.get_field(tbl_name, name) {
            let mut def = format!("DEFINE FIELD {} ON {}", name, tbl_name);
            if let Some(ref typ) = field.typ {
                def.push_str(&format!(" TYPE {}", display_kind(typ)));
            }
            if field.readonly {
                def.push_str(" READONLY");
            }
            matches.push(format!("```surql\n{def}\n```"));
        }
    }

    if matches.is_empty() {
        return None;
    }

    let value = matches.join("\n\n---\n\n");
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(range),
    })
}

/// Show what `*` expands to.
fn wildcard_hover(table: Option<&str>, ctx: &Context, range: Range) -> Option<Hover> {
    let tbl = table?;
    let fields = ctx.get_fields(tbl);
    if fields.is_empty() {
        return None;
    }

    let mut code_lines = vec![format!("-- SELECT * FROM {tbl} expands to:")];
    for field in fields {
        let mut field_def = format!("{}", field.name);
        if let Some(ref typ) = field.typ {
            field_def.push_str(&format!(": {}", display_kind(typ)));
        }
        code_lines.push(format!("  {}", field_def));
    }
    let value = format!("```surql\n{}\n```", code_lines.join("\n"));

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(range),
    })
}

/// Show a variable's inferred type.
fn variable_hover(name: &str, ctx: &Context, range: Range) -> Option<Hover> {
    let var_name = if name.starts_with('$') {
        name.to_string()
    } else {
        format!("${name}")
    };

    let binding = ctx.scope.lookup(&var_name)?;
    if binding.typ.is_any() {
        return None;
    }

    let type_str = format_type_for_hover(&binding.typ);
    let value = format!("```surql\n{var_name}: {type_str}\n```");

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(range),
    })
}

/// Show a builtin function's signature and docs.
fn builtin_function_hover(name: &str, range: Range) -> Option<Hover> {
    // Check if this looks like a function name (contains ::)
    let full_name = name;

    // Look up in our known builtins
    let func = BUILTIN_FUNCTIONS.iter().find(|f| f.name == full_name)?;

    let anchor = func.doc_anchor;
    let url = format!(
        "https://surrealdb.com/docs/surrealql/functions/database/{}#{}",
        func.doc_namespace, anchor
    );

    let value = format!(
        "```surql\n{}\n```\n\n{}\n\n[SurrealDB docs]({})",
        func.signature, func.summary, url
    );

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(range),
    })
}

/// Find the relation table name preceding a target in a graph path.
/// In `->wrote->user`, the tree is:
///   path
///     graph_path "->wrote"
///       identifier "wrote"
///     path_element "->user"
///       graph_path "->user"
///         identifier "user"
///
/// So from "user", we need to go up to path_element, then look at the
/// previous sibling (graph_path containing "wrote").
fn find_preceding_relation(node: &tree_sitter::Node, source: &str) -> Option<String> {
    // Walk up to find the path or path_element level
    let mut current = *node;
    while let Some(parent) = current.parent() {
        if parent.kind() == "path" {
            // We're inside a path — find our position and look backwards
            let mut cursor = parent.walk();
            let children: Vec<_> = parent.named_children(&mut cursor).collect();
            let our_idx = children.iter().position(|c| {
                // Check if this child contains our node
                c.start_byte() <= node.start_byte() && c.end_byte() >= node.end_byte()
            })?;
            // Look at the previous sibling
            if our_idx > 0 {
                return first_identifier(&children[our_idx - 1], source);
            }
            return None;
        }
        current = parent;
    }
    None
}

/// Walk up from a node to find the enclosing statement's target table.
fn find_statement_table(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let mut current = *node;
    while let Some(parent) = current.parent() {
        if parent.kind().ends_with("_statement") {
            return extract_table_from_statement(&parent, source);
        }
        current = parent;
    }
    None
}

/// Format a type for hover display. Object types with multiple fields
/// are rendered multi-line for readability.
fn format_type_for_hover(kind: &Kind) -> String {
    match kind {
        Kind::Literal(Literal::Object(fields)) if fields.len() > 2 => {
            let mut lines = vec!["{".to_string()];
            for (name, typ) in fields {
                lines.push(format!("  {}: {},", name, format_type_for_hover(typ)));
            }
            lines.push("}".to_string());
            lines.join("\n")
        }
        Kind::Array(inner, _) => {
            format!("array<{}>", format_type_for_hover(inner))
        }
        Kind::Set(inner, _) => {
            format!("set<{}>", format_type_for_hover(inner))
        }
        Kind::Option(inner) => {
            format!("option<{}>", format_type_for_hover(inner))
        }
        _ => display_kind(kind),
    }
}

/// Extract the target table name from a statement node.
fn extract_table_from_statement(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();

    match node.kind() {
        "select_statement" => children
            .iter()
            .find(|c| c.kind() == "from_clause")
            .and_then(|from| first_identifier(from, source)),
        "create_statement" => children
            .iter()
            .find(|c| c.kind() == "create_target")
            .and_then(|target| first_identifier(target, source)),
        "update_statement" | "delete_statement" | "upsert_statement" => children
            .iter()
            .find(|c| c.kind() == "identifier")
            .and_then(|c| c.utf8_text(source.as_bytes()).ok().map(|s| s.trim().to_string())),
        _ => None,
    }
}

/// Find the first identifier descendant and return its text.
fn first_identifier(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let mut stack = vec![*node];
    while let Some(current) = stack.pop() {
        if current.kind() == "identifier" {
            return current.utf8_text(source.as_bytes()).ok().map(|s| s.trim().to_string());
        }
        let mut cursor = current.walk();
        for child in current.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    None
}

/// Check if an identifier is a statement target (before any clause).
fn is_target_identifier(statement: &tree_sitter::Node, ident: &tree_sitter::Node) -> bool {
    let mut cursor = statement.walk();
    for child in statement.named_children(&mut cursor) {
        if child.kind().ends_with("_clause") {
            return false; // We've passed into clauses, this ident isn't the target
        }
        if child.id() == ident.id() {
            return true;
        }
    }
    false
}

// ── Built-in function database ──────────────────────────────────────────

static BUILTIN_FUNCTIONS: &[BuiltinFn] = &[
    // String functions
    BuiltinFn { name: "string::len", signature: "string::len(value: string) -> number", summary: "Returns the length of a string in characters.", doc_anchor: "stringlen", doc_namespace: "string" },
    BuiltinFn { name: "string::lowercase", signature: "string::lowercase(value: string) -> string", summary: "Converts a string to lowercase.", doc_anchor: "stringlowercase", doc_namespace: "string" },
    BuiltinFn { name: "string::uppercase", signature: "string::uppercase(value: string) -> string", summary: "Converts a string to uppercase.", doc_anchor: "stringuppercase", doc_namespace: "string" },
    BuiltinFn { name: "string::trim", signature: "string::trim(value: string) -> string", summary: "Removes whitespace from the start and end of a string.", doc_anchor: "stringtrim", doc_namespace: "string" },
    BuiltinFn { name: "string::capitalize", signature: "string::capitalize(value: string) -> string", summary: "Capitalizes the first character of each word.", doc_anchor: "stringcapitalize", doc_namespace: "string" },
    BuiltinFn { name: "string::concat", signature: "string::concat(value...) -> string", summary: "Concatenates values into a single string.", doc_anchor: "stringconcat", doc_namespace: "string" },
    BuiltinFn { name: "string::contains", signature: "string::contains(value: string, search: string) -> bool", summary: "Checks whether a string contains another string.", doc_anchor: "stringcontains", doc_namespace: "string" },
    BuiltinFn { name: "string::ends_with", signature: "string::ends_with(value: string, suffix: string) -> bool", summary: "Checks whether a string ends with another string.", doc_anchor: "stringendswith", doc_namespace: "string" },
    BuiltinFn { name: "string::starts_with", signature: "string::starts_with(value: string, prefix: string) -> bool", summary: "Checks whether a string starts with another string.", doc_anchor: "stringstartswith", doc_namespace: "string" },
    BuiltinFn { name: "string::join", signature: "string::join(delimiter: string, value...) -> string", summary: "Joins values together with a delimiter.", doc_anchor: "stringjoin", doc_namespace: "string" },
    BuiltinFn { name: "string::repeat", signature: "string::repeat(value: string, count: number) -> string", summary: "Repeats a string a given number of times.", doc_anchor: "stringrepeat", doc_namespace: "string" },
    BuiltinFn { name: "string::replace", signature: "string::replace(value: string, search: string, replace: string) -> string", summary: "Replaces occurrences of a string with another.", doc_anchor: "stringreplace", doc_namespace: "string" },
    BuiltinFn { name: "string::reverse", signature: "string::reverse(value: string) -> string", summary: "Reverses a string.", doc_anchor: "stringreverse", doc_namespace: "string" },
    BuiltinFn { name: "string::slice", signature: "string::slice(value: string, start: number, end: number) -> string", summary: "Extracts a section of a string.", doc_anchor: "stringslice", doc_namespace: "string" },
    BuiltinFn { name: "string::slug", signature: "string::slug(value: string) -> string", summary: "Converts a string to a URL-safe slug.", doc_anchor: "stringslug", doc_namespace: "string" },
    BuiltinFn { name: "string::split", signature: "string::split(value: string, delimiter: string) -> array", summary: "Splits a string by a delimiter.", doc_anchor: "stringsplit", doc_namespace: "string" },
    BuiltinFn { name: "string::words", signature: "string::words(value: string) -> array", summary: "Splits a string into individual words.", doc_anchor: "stringwords", doc_namespace: "string" },
    // Array functions
    BuiltinFn { name: "array::len", signature: "array::len(value: array) -> number", summary: "Returns the number of items in an array.", doc_anchor: "arraylen", doc_namespace: "array" },
    BuiltinFn { name: "array::push", signature: "array::push(array: array, value: any) -> array", summary: "Appends a value to the end of an array.", doc_anchor: "arraypush", doc_namespace: "array" },
    BuiltinFn { name: "array::pop", signature: "array::pop(array: array) -> any", summary: "Removes and returns the last item from an array.", doc_anchor: "arraypop", doc_namespace: "array" },
    BuiltinFn { name: "array::sort", signature: "array::sort(array: array) -> array", summary: "Sorts the items in an array.", doc_anchor: "arraysort", doc_namespace: "array" },
    BuiltinFn { name: "array::reverse", signature: "array::reverse(array: array) -> array", summary: "Reverses the items in an array.", doc_anchor: "arrayreverse", doc_namespace: "array" },
    BuiltinFn { name: "array::flatten", signature: "array::flatten(array: array) -> array", summary: "Flattens nested arrays into a single array.", doc_anchor: "arrayflatten", doc_namespace: "array" },
    BuiltinFn { name: "array::distinct", signature: "array::distinct(array: array) -> array", summary: "Returns unique items from an array.", doc_anchor: "arraydistinct", doc_namespace: "array" },
    BuiltinFn { name: "array::filter", signature: "array::filter(array: array, closure: fn) -> array", summary: "Filters items using a closure.", doc_anchor: "arrayfilter", doc_namespace: "array" },
    BuiltinFn { name: "array::map", signature: "array::map(array: array, closure: fn) -> array", summary: "Transforms each item using a closure.", doc_anchor: "arraymap", doc_namespace: "array" },
    BuiltinFn { name: "array::find", signature: "array::find(array: array, closure: fn) -> any", summary: "Returns the first item matching a closure.", doc_anchor: "arrayfind", doc_namespace: "array" },
    // Math functions
    BuiltinFn { name: "math::abs", signature: "math::abs(value: number) -> number", summary: "Returns the absolute value of a number.", doc_anchor: "mathabs", doc_namespace: "math" },
    BuiltinFn { name: "math::ceil", signature: "math::ceil(value: number) -> number", summary: "Rounds a number up to the next integer.", doc_anchor: "mathceil", doc_namespace: "math" },
    BuiltinFn { name: "math::floor", signature: "math::floor(value: number) -> number", summary: "Rounds a number down to the previous integer.", doc_anchor: "mathfloor", doc_namespace: "math" },
    BuiltinFn { name: "math::round", signature: "math::round(value: number) -> number", summary: "Rounds a number to the nearest integer.", doc_anchor: "mathround", doc_namespace: "math" },
    BuiltinFn { name: "math::sum", signature: "math::sum(array: array) -> number", summary: "Returns the sum of all values in an array.", doc_anchor: "mathsum", doc_namespace: "math" },
    BuiltinFn { name: "math::mean", signature: "math::mean(array: array) -> number", summary: "Returns the average of all values.", doc_anchor: "mathmean", doc_namespace: "math" },
    BuiltinFn { name: "math::max", signature: "math::max(array: array) -> number", summary: "Returns the maximum value.", doc_anchor: "mathmax", doc_namespace: "math" },
    BuiltinFn { name: "math::min", signature: "math::min(array: array) -> number", summary: "Returns the minimum value.", doc_anchor: "mathmin", doc_namespace: "math" },
    // Crypto functions
    BuiltinFn { name: "crypto::md5", signature: "crypto::md5(value: string) -> string", summary: "Returns the MD5 hash of a string.", doc_anchor: "cryptomd5", doc_namespace: "crypto" },
    BuiltinFn { name: "crypto::sha256", signature: "crypto::sha256(value: string) -> string", summary: "Returns the SHA-256 hash of a string.", doc_anchor: "cryptosha256", doc_namespace: "crypto" },
    BuiltinFn { name: "crypto::sha512", signature: "crypto::sha512(value: string) -> string", summary: "Returns the SHA-512 hash of a string.", doc_anchor: "cryptosha512", doc_namespace: "crypto" },
    // Type functions
    BuiltinFn { name: "type::is::string", signature: "type::is::string(value: any) -> bool", summary: "Checks if a value is a string.", doc_anchor: "typeisstring", doc_namespace: "type" },
    BuiltinFn { name: "type::is::int", signature: "type::is::int(value: any) -> bool", summary: "Checks if a value is an integer.", doc_anchor: "typeisint", doc_namespace: "type" },
    BuiltinFn { name: "type::is::number", signature: "type::is::number(value: any) -> bool", summary: "Checks if a value is a number.", doc_anchor: "typeisnumber", doc_namespace: "type" },
    BuiltinFn { name: "type::is::bool", signature: "type::is::bool(value: any) -> bool", summary: "Checks if a value is a boolean.", doc_anchor: "typeisbool", doc_namespace: "type" },
    // Count
    BuiltinFn { name: "count", signature: "count(value: any) -> number", summary: "Counts the number of items.", doc_anchor: "count", doc_namespace: "count" },
];
