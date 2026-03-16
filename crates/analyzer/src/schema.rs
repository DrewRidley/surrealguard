/// Schema extraction from tree-sitter CST.
///
/// Walks DEFINE TABLE, DEFINE FIELD, DEFINE INDEX, and DEFINE FUNCTION
/// statements and populates the [`Context`] with structured definitions.
use tree_sitter::Node;

use crate::context::{
    Context, FieldDef, FunctionDef, IndexDef, PermissionLevel, SchemaMode, TableDef, TableKind,
};
use crate::diagnostic::{Code, Diagnostic};
use crate::parser::{child_by_kind, children_by_kind, find_all, node_text};
use crate::span::Span;
use crate::types::{Kind, Table};

/// Extract all schema definitions from a parsed CST and add them to the context.
pub fn extract_schema(root: &Node, source: &str, ctx: &mut Context) {
    extract_tables(root, source, ctx);
    extract_fields(root, source, ctx);
    validate_nested_fields(ctx);
    validate_reference_fields(ctx);
    validate_conflicting_permissions(ctx);
    extract_indexes(root, source, ctx);
    extract_functions(root, source, ctx);
}

// ── Tables ────────────────────────────────────────────────────

fn extract_tables(root: &Node, source: &str, ctx: &mut Context) {
    for node in find_all(root, "define_table_statement") {
        if let Some(def) = parse_table_def(&node, source) {
            let span = Span::from_node(&node);
            let name = &def.name;
            let has_overwrite = child_by_kind(&node, "keyword_overwrite").is_some();
            let has_if_not_exists = child_by_kind(&node, "if_not_exists_clause").is_some();
            if let Some(prev) = ctx.get_table(name) {
                if has_if_not_exists {
                    // IF NOT EXISTS — silently skip, no error
                } else if has_overwrite {
                    // OVERWRITE — allowed, just a hint
                    let prev_span = prev.span;
                    ctx.emit(Diagnostic::hint(
                        span,
                        Code::DuplicateTableDef,
                        format!("table `{}` is being overwritten", name),
                    )
                    .with_related(prev_span, format!("previous definition of `{}` here", name)));
                } else {
                    // No OVERWRITE — this is an error in SurrealDB
                    let prev_span = prev.span;
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::DuplicateTableDef,
                        format!("table `{}` is already defined", name),
                    )
                    .with_related(prev_span, format!("previous definition of `{}` here", name))
                    .with_suggestion("use `DEFINE TABLE OVERWRITE` to replace the existing definition"));
                }
            }
            ctx.define_table(def);
        }
    }
}

fn parse_table_def(node: &Node, source: &str) -> Option<TableDef> {
    let name = find_table_name(node, source)?;
    let span = Span::from_node(node);

    let has_keyword = |kind: &str| child_by_kind(node, kind).is_some();

    let schema_mode = if has_keyword("keyword_schemafull") {
        SchemaMode::Schemafull
    } else {
        SchemaMode::Schemaless
    };

    let drop = has_keyword("keyword_drop");

    let kind = if let Some(type_clause) = child_by_kind(node, "table_type_clause") {
        parse_table_kind(&type_clause, source)
    } else {
        TableKind::Normal
    };

    let permissions = child_by_kind(node, "permissions_for_clause")
        .map(|perm_node| parse_permission_level(&perm_node));

    Some(TableDef {
        name,
        span,
        schema_mode,
        kind,
        drop,
        permissions,
    })
}

fn find_table_name(node: &Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            return Some(node_text(&child, source).to_string());
        }
    }
    None
}

fn parse_table_kind(node: &Node, source: &str) -> TableKind {
    if child_by_kind(node, "keyword_any").is_some() {
        return TableKind::Any;
    }
    if child_by_kind(node, "keyword_normal").is_some() {
        return TableKind::Normal;
    }
    if child_by_kind(node, "keyword_relation").is_some() {
        let from = extract_relation_tables(node, source, true);
        let to = extract_relation_tables(node, source, false);
        return TableKind::Relation { from, to };
    }
    TableKind::Normal
}

fn extract_relation_tables(node: &Node, source: &str, is_from: bool) -> Option<Vec<String>> {
    let mut cursor = node.walk();
    let mut found_direction = false;
    for child in node.children(&mut cursor) {
        if !found_direction {
            let kind = child.kind();
            if is_from && (kind == "keyword_in" || kind == "keyword_from") {
                found_direction = true;
            } else if !is_from && (kind == "keyword_out" || kind == "keyword_to") {
                found_direction = true;
            }
        } else if child.kind() == "record_or_separated" {
            let tables = extract_identifiers_from_separated(&child, source);
            return Some(tables);
        }
    }
    None
}

fn extract_identifiers_from_separated(node: &Node, source: &str) -> Vec<String> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|c| c.kind() == "identifier")
        .map(|c| node_text(&c, source).to_string())
        .collect()
}

// ── Fields ────────────────────────────────────────────────────

fn extract_fields(root: &Node, source: &str, ctx: &mut Context) {
    for node in find_all(root, "define_field_statement") {
        if let Some(def) = parse_field_def(&node, source) {
            let span = Span::from_node(&node);
            let table_name = &def.table;
            let field_name = &def.name;
            if !table_name.is_empty() && !ctx.has_table(table_name) && ctx.strict {
                ctx.emit(Diagnostic::warning(
                    span,
                    Code::FieldOnUndefinedTable,
                    format!("field `{}` defined on table `{}`, but `{}` has not been defined", field_name, table_name, table_name),
                )
                .with_suggestion(format!("add `DEFINE TABLE {} SCHEMAFULL;` before this field definition", table_name)));
            }
            if let Some(prev_field) = ctx.get_field(table_name, field_name) {
                let has_overwrite = child_by_kind(&node, "keyword_overwrite").is_some();
                let has_if_not_exists = child_by_kind(&node, "if_not_exists_clause").is_some();
                let prev_span = prev_field.span;
                if has_if_not_exists {
                    // IF NOT EXISTS — silently skip
                } else if has_overwrite {
                    ctx.emit(Diagnostic::hint(
                        span,
                        Code::DuplicateFieldDef,
                        format!("field `{}` on table `{}` is being overwritten", field_name, table_name),
                    )
                    .with_related(prev_span, format!("previous definition of `{}` here", field_name)));
                } else {
                    ctx.emit(Diagnostic::error(
                        span,
                        Code::DuplicateFieldDef,
                        format!("field `{}` on table `{}` is already defined", field_name, table_name),
                    )
                    .with_related(prev_span, format!("previous definition of `{}` here", field_name))
                    .with_suggestion("use `DEFINE FIELD OVERWRITE` to replace the existing definition"));
                }
            }
            // Validate record<table_name> references in strict mode
            if ctx.strict {
                if let Some(ref typ) = def.typ {
                    check_record_refs(typ, span, ctx);
                }
            }
            ctx.define_field(def);
        }
    }
}

/// Recursively check a type for `record<table_name>` references to undefined tables.
fn check_record_refs(typ: &Kind, span: Span, ctx: &mut Context) {
    match typ {
        Kind::Record(tables) => {
            for table in tables {
                let table_name = table.to_string();
                if !table_name.is_empty() && !ctx.has_table(&table_name) {
                    ctx.emit(Diagnostic::warning(
                        span,
                        Code::RecordRefUndefinedTable,
                        format!(
                            "record type references table `{}` which has not been defined",
                            table_name
                        ),
                    ));
                }
            }
        }
        Kind::Array(inner, _) | Kind::Set(inner, _) | Kind::Option(inner) => {
            check_record_refs(inner, span, ctx);
        }
        Kind::Either(variants) => {
            for v in variants {
                check_record_refs(v, span, ctx);
            }
        }
        _ => {}
    }
}

fn parse_field_def(node: &Node, source: &str) -> Option<FieldDef> {
    let table = find_field_table(node, source)?;
    let name = find_field_name(node, source)?;
    let span = Span::from_node(node);

    let typ = child_by_kind(node, "type_clause")
        .and_then(|tc| parse_type_from_clause(&tc, source));

    let readonly = child_by_kind(node, "readonly_clause").is_some();
    let is_computed = child_by_kind(node, "computed_clause").is_some();
    let flexible = child_by_kind(node, "keyword_flexible").is_some();

    let default = child_by_kind(node, "default_clause")
        .map(|dc| node_text(&dc, source).to_string());
    let has_assert = child_by_kind(node, "assert_clause").is_some();
    let reference = child_by_kind(node, "reference_clause").is_some();

    let permissions = child_by_kind(node, "permissions_for_clause")
        .map(|perm_node| parse_permission_level(&perm_node));

    Some(FieldDef {
        name,
        table,
        span,
        typ,
        default,
        readonly,
        is_computed,
        flexible,
        has_assert,
        reference,
        permissions,
    })
}

fn find_field_table(node: &Node, source: &str) -> Option<String> {
    let on_clause = child_by_kind(node, "on_table_clause")?;
    let ident = child_by_kind(&on_clause, "identifier")?;
    Some(node_text(&ident, source).to_string())
}

fn find_field_name(node: &Node, source: &str) -> Option<String> {
    let pred = child_by_kind(node, "inclusive_predicate")?;
    let identifiers = find_all(&pred, "identifier");
    if identifiers.is_empty() {
        return None;
    }
    // Join all identifiers with dots to handle nested field names like `address.city`
    let name = identifiers
        .iter()
        .map(|id| node_text(id, source))
        .collect::<Vec<_>>()
        .join(".");
    Some(name)
}

// ── Nested Field Validation ───────────────────────────────────

/// Validate that nested fields (e.g., `address.city`) have their parent
/// field defined as an object type. Emits a warning for each nested field
/// whose immediate parent is missing or not typed as `object`.
fn validate_nested_fields(ctx: &mut Context) {
    // Collect all (table, field_name, span) triples for nested fields first,
    // to avoid borrowing ctx while iterating.
    let nested_fields: Vec<(String, String, Span)> = ctx
        .field_table_names()
        .into_iter()
        .flat_map(|table| {
            ctx.get_fields(&table)
                .iter()
                .filter(|f| f.name.contains('.'))
                .map(|f| (f.table.clone(), f.name.clone(), f.span))
                .collect::<Vec<_>>()
        })
        .collect();

    for (table, field_name, span) in nested_fields {
        if let Some(dot_pos) = field_name.rfind('.') {
            let parent_name = &field_name[..dot_pos];
            let parent_ok = ctx
                .get_field(&table, parent_name)
                .map(|parent| {
                    match &parent.typ {
                        Some(Kind::Object) => true,
                        // No type annotation — could be object, don't warn
                        None => true,
                        _ => false,
                    }
                })
                .unwrap_or(false);

            if !parent_ok {
                let has_parent = ctx.get_field(&table, parent_name).is_some();
                let msg = if has_parent {
                    format!(
                        "nested field `{}` on table `{}` has parent `{}` which is not defined as type `object`",
                        field_name, table, parent_name
                    )
                } else {
                    format!(
                        "nested field `{}` on table `{}` has no parent field `{}` defined; add `DEFINE FIELD {} ON {} TYPE object`",
                        field_name, table, parent_name, parent_name, table
                    )
                };
                ctx.emit(Diagnostic::warning(
                    span,
                    Code::NestedFieldWithoutParent,
                    msg,
                ));
            }
        }
    }
}

// ── REFERENCE Field Validation ─────────────────────────────────

/// Validate that fields with the REFERENCE clause have a `record<T>` type.
///
/// The REFERENCE clause indicates a foreign key relationship and only makes
/// sense on fields typed as `record<T>`. Using REFERENCE on a non-record
/// field (e.g., `string`, `int`) is almost certainly a mistake.
///
/// In strict mode, also validates that the referenced table(s) exist.
fn validate_reference_fields(ctx: &mut Context) {
    let checks: Vec<(String, String, Span, Option<Kind>)> = ctx
        .field_table_names()
        .into_iter()
        .flat_map(|table| {
            ctx.get_fields(&table)
                .iter()
                .filter(|f| f.reference)
                .map(|f| (f.table.clone(), f.name.clone(), f.span, f.typ.clone()))
                .collect::<Vec<_>>()
        })
        .collect();

    for (table, field_name, span, typ) in checks {
        let is_record = match &typ {
            Some(Kind::Record(_)) => true,
            Some(Kind::Option(inner)) => matches!(**inner, Kind::Record(_)),
            _ => false,
        };

        if !is_record {
            let type_str = typ
                .as_ref()
                .map(|t| format!("`{}`", t))
                .unwrap_or_else(|| "no type".to_string());
            ctx.emit(Diagnostic::warning(
                span,
                Code::ReferenceOnNonRecord,
                format!(
                    "REFERENCE clause on field `{}` of table `{}` requires a `record<T>` type, but field has {}",
                    field_name, table, type_str
                ),
            ));
        } else if ctx.strict {
            // In strict mode, validate that referenced tables exist
            if let Some(ref kind) = typ {
                check_record_refs(kind, span, ctx);
            }
        }
    }
}

// ── Permission Parsing ────────────────────────────────────────

/// Determine the overall permission level from a `permissions_for_clause` node.
///
/// The grammar is:
///   PERMISSIONS (NONE | FULL | (FOR action[, action]... (WHERE ... | NONE | FULL))+)
///
/// When `PERMISSIONS NONE` or `PERMISSIONS FULL` is used directly, we return
/// that level. When per-action clauses are used, we return `Custom`.
fn parse_permission_level(node: &Node) -> PermissionLevel {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "keyword_none" => return PermissionLevel::None,
            "keyword_full" => return PermissionLevel::Full,
            _ => {}
        }
    }
    // If we didn't find a top-level NONE/FULL, it uses per-action clauses
    PermissionLevel::Custom
}

// ── Permission Conflict Validation ────────────────────────────

/// Check for conflicting table-level vs field-level permissions.
///
/// For example, if a table has `PERMISSIONS NONE` but a field on that table
/// has `PERMISSIONS FULL`, the field permission is effectively dead code because
/// the table-level permission blocks all access first. This is likely a mistake.
///
/// Similarly, if a table has `PERMISSIONS FULL` but a field has `PERMISSIONS NONE`,
/// the field restriction may not behave as expected since table-level already
/// grants full access.
fn validate_conflicting_permissions(ctx: &mut Context) {
    // Collect all data we need first to avoid borrowing ctx during iteration.
    let checks: Vec<(String, PermissionLevel, Span, Vec<(String, PermissionLevel, Span)>)> = ctx
        .table_names()
        .map(|name| name.to_string())
        .collect::<Vec<_>>()
        .into_iter()
        .filter_map(|table_name| {
            let table = ctx.get_table(&table_name)?;
            let table_perm = table.permissions?;
            let table_span = table.span;

            let field_perms: Vec<(String, PermissionLevel, Span)> = ctx
                .get_fields(&table_name)
                .iter()
                .filter_map(|f| {
                    let fp = f.permissions?;
                    Some((f.name.clone(), fp, f.span))
                })
                .collect();

            if field_perms.is_empty() {
                return None;
            }

            Some((table_name, table_perm, table_span, field_perms))
        })
        .collect();

    for (table_name, table_perm, table_span, field_perms) in checks {
        for (field_name, field_perm, field_span) in &field_perms {
            let conflict = match (table_perm, field_perm) {
                (PermissionLevel::None, PermissionLevel::Full) => Some((
                    "NONE",
                    "FULL",
                    "table-level PERMISSIONS NONE blocks all access, so field-level FULL has no effect",
                )),
                (PermissionLevel::None, PermissionLevel::Custom) => Some((
                    "NONE",
                    "a WHERE clause",
                    "table-level PERMISSIONS NONE blocks all access, so field-level permissions have no effect",
                )),
                (PermissionLevel::Full, PermissionLevel::None) => Some((
                    "FULL",
                    "NONE",
                    "table-level PERMISSIONS FULL grants unrestricted access; field-level NONE may not restrict as expected",
                )),
                _ => None,
            };

            if let Some((table_level, field_level, explanation)) = conflict {
                ctx.emit(
                    Diagnostic::warning(
                        *field_span,
                        Code::ConflictingPermissions,
                        format!(
                            "field `{}` on table `{}` has PERMISSIONS {} but table has PERMISSIONS {}; {}",
                            field_name, table_name, field_level, table_level, explanation
                        ),
                    )
                    .with_related(table_span, format!("table `{}` permissions defined here", table_name)),
                );
            }
        }
    }
}

// ── Indexes ───────────────────────────────────────────────────

fn extract_indexes(root: &Node, source: &str, ctx: &mut Context) {
    for node in find_all(root, "define_index_statement") {
        if let Some(def) = parse_index_def(&node, source) {
            let span = Span::from_node(&node);
            let table_name = &def.table;
            let fields = &def.fields;
            if ctx.is_schemafull(table_name) {
                for field in fields {
                    if ctx.get_field(table_name, field).is_none() {
                        ctx.emit(Diagnostic::warning(
                            span,
                            Code::IndexOnUndefinedField,
                            format!("index field `{}` not defined on schemafull table `{}`", field, table_name),
                        ));
                    }
                }
            }
            ctx.define_index(def);
        }
    }
}

fn parse_index_def(node: &Node, source: &str) -> Option<IndexDef> {
    let mut cursor = node.walk();

    // Find index name (first identifier)
    let name_node = node
        .children(&mut cursor)
        .find(|c| c.kind() == "identifier")?;
    let name = node_text(&name_node, source).to_string();
    let span = Span::from_node(node);

    // Find table from ON clause
    let on_clause = child_by_kind(node, "on_table_clause")?;
    let table_ident = child_by_kind(&on_clause, "identifier")?;
    let table = node_text(&table_ident, source).to_string();

    // Find fields from FIELDS/COLUMNS clause
    let fields = if let Some(fields_clause) = child_by_kind(node, "fields_columns_clause") {
        find_all(&fields_clause, "identifier")
            .iter()
            .map(|n| node_text(n, source).to_string())
            .collect()
    } else {
        vec![]
    };

    let unique = child_by_kind(node, "keyword_unique").is_some();

    Some(IndexDef {
        name,
        table,
        span,
        fields,
        unique,
    })
}

// ── Functions ─────────────────────────────────────────────────

fn extract_functions(root: &Node, source: &str, ctx: &mut Context) {
    for node in find_all(root, "define_function_statement") {
        if let Some(def) = parse_function_def(&node, source) {
            ctx.define_function(def);
        }
    }
}

fn parse_function_def(node: &Node, source: &str) -> Option<FunctionDef> {
    let span = Span::from_node(node);

    // Find function name (custom_function_name or identifier after fn::)
    let name = if let Some(fn_name) = child_by_kind(node, "custom_function_name") {
        node_text(&fn_name, source).to_string()
    } else {
        // Fallback: look for identifier
        let mut cursor = node.walk();
        let ident = node
            .children(&mut cursor)
            .find(|c| c.kind() == "identifier")?;
        node_text(&ident, source).to_string()
    };

    // Parse parameters
    let params = if let Some(param_list) = child_by_kind(node, "param_list") {
        parse_param_list(&param_list, source)
    } else {
        vec![]
    };

    // Parse return type if present
    let return_type = child_by_kind(node, "function_return_type")
        .and_then(|rt| child_by_kind(&rt, "type"))
        .and_then(|t| parse_type(&t, source));

    Some(FunctionDef {
        name,
        span,
        params,
        return_type,
    })
}

fn parse_param_list(node: &Node, source: &str) -> Vec<(String, Kind)> {
    let mut params = Vec::new();
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();

    // Params can be structured as:
    // 1. param_definition { variable_name, type } — wrapped
    // 2. variable_name, type — flat (sequential children)
    let mut i = 0;
    while i < children.len() {
        let child = &children[i];
        match child.kind() {
            "param_definition" | "variable" => {
                let var_name = node_text(child, source).to_string();
                let typ = child_by_kind(child, "type")
                    .and_then(|t| parse_type(&t, source))
                    .unwrap_or(Kind::Any);
                params.push((var_name, typ));
                i += 1;
            }
            "variable_name" => {
                let var_name = node_text(child, source).to_string();
                // Next sibling might be the type
                let typ = if i + 1 < children.len() && children[i + 1].kind() == "type" {
                    i += 1; // consume the type node
                    parse_type(&children[i], source).unwrap_or(Kind::Any)
                } else {
                    Kind::Any
                };
                params.push((var_name, typ));
                i += 1;
            }
            _ => { i += 1; }
        }
    }
    params
}

// ── Type Parsing ──────────────────────────────────────────────

fn parse_type_from_clause(node: &Node, source: &str) -> Option<Kind> {
    let type_node = child_by_kind(node, "type")?;
    parse_type(&type_node, source)
}

/// Parse a `type` node into our Type representation.
pub fn parse_type(node: &Node, source: &str) -> Option<Kind> {
    match node.kind() {
        "type" => {
            let mut cursor = node.walk();
            let child = node.named_children(&mut cursor).next()?;
            parse_type(&child, source)
        }
        "type_name" => {
            let name = node_text(node, source);
            Some(type_name_to_type(name))
        }
        "parameterized_type" => {
            let name_node = child_by_kind(node, "type_name")?;
            let name = node_text(&name_node, source);
            let type_args: Vec<Kind> = children_by_kind(node, "type")
                .iter()
                .filter_map(|t| parse_type(t, source))
                .collect();

            match name {
                "array" => {
                    let inner = type_args.into_iter().next().unwrap_or(Kind::Any);
                    Some(Kind::Array(Box::new(inner), None))
                }
                "set" => {
                    let inner = type_args.into_iter().next().unwrap_or(Kind::Any);
                    Some(Kind::Set(Box::new(inner), None))
                }
                "option" => {
                    let inner = type_args.into_iter().next().unwrap_or(Kind::Any);
                    Some(Kind::Option(Box::new(inner)))
                }
                "record" => {
                    let tables: Vec<Table> = children_by_kind(node, "type")
                        .iter()
                        .filter_map(|t| {
                            let tn = child_by_kind(t, "type_name")?;
                            Some(Table::from(node_text(&tn, source).to_string()))
                        })
                        .collect();
                    Some(Kind::Record(tables))
                }
                "geometry" => {
                    let variants: Vec<String> = children_by_kind(node, "type")
                        .iter()
                        .filter_map(|t| {
                            let tn = child_by_kind(t, "type_name")?;
                            Some(node_text(&tn, source).to_string())
                        })
                        .collect();
                    Some(Kind::Geometry(variants))
                }
                _ => Some(Kind::Any),
            }
        }
        "union_type" => {
            let mut cursor = node.walk();
            let types: Vec<Kind> = node
                .named_children(&mut cursor)
                .filter_map(|child| parse_type(&child, source))
                .collect();
            if types.len() == 1 {
                Some(types.into_iter().next().unwrap())
            } else {
                Some(Kind::Either(types))
            }
        }
        _ => None,
    }
}

fn type_name_to_type(name: &str) -> Kind {
    match name {
        "any" => Kind::Any,
        "null" | "none" => Kind::Null,
        "bool" => Kind::Bool,
        "int" => Kind::Int,
        "float" => Kind::Float,
        "decimal" => Kind::Decimal,
        "number" => Kind::Number,
        "string" => Kind::String,
        "bytes" => Kind::Bytes,
        "duration" => Kind::Duration,
        "datetime" => Kind::Datetime,
        "uuid" => Kind::Uuid,
        "object" => Kind::Object,
        "range" => Kind::Range,
        _ => Kind::Any,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn ctx_from(source: &str) -> Context {
        let tree = parser::parse(source).unwrap();
        let root = tree.root_node();
        let mut ctx = Context::new();
        extract_schema(&root, source, &mut ctx);
        ctx
    }

    #[test]
    fn extract_schemafull_table() {
        let ctx = ctx_from("DEFINE TABLE user SCHEMAFULL;");
        let table = ctx.get_table("user").unwrap();
        assert_eq!(table.name, "user");
        assert_eq!(table.schema_mode, SchemaMode::Schemafull);
        assert!(!table.drop);
    }

    #[test]
    fn extract_drop_table() {
        let ctx = ctx_from("DEFINE TABLE temp DROP SCHEMALESS;");
        let table = ctx.get_table("temp").unwrap();
        assert!(table.drop);
        assert_eq!(table.schema_mode, SchemaMode::Schemaless);
    }

    #[test]
    fn extract_relation_table() {
        let ctx = ctx_from("DEFINE TABLE knows TYPE RELATION IN person OUT person;");
        let table = ctx.get_table("knows").unwrap();
        if let TableKind::Relation { from, to } = &table.kind {
            assert_eq!(from.as_ref().unwrap(), &["person"]);
            assert_eq!(to.as_ref().unwrap(), &["person"]);
        } else {
            panic!("Expected relation table kind");
        }
    }

    #[test]
    fn extract_field_string_type() {
        let ctx = ctx_from(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;",
        );
        let field = ctx.get_field("user", "name").unwrap();
        assert_eq!(field.typ, Some(Kind::String));
    }

    #[test]
    fn extract_field_record_type() {
        let ctx = ctx_from(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD org ON user TYPE record<organization>;",
        );
        let field = ctx.get_field("user", "org").unwrap();
        assert_eq!(
            field.typ,
            Some(Kind::Record(vec![Table::from("organization".to_string())]))
        );
    }

    #[test]
    fn extract_field_option_array() {
        let ctx = ctx_from(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD tags ON user TYPE option<array<string>>;",
        );
        let field = ctx.get_field("user", "tags").unwrap();
        assert_eq!(
            field.typ,
            Some(Kind::Option(Box::new(Kind::Array(
                Box::new(Kind::String),
                None
            ))))
        );
    }

    #[test]
    fn build_full_table_type() {
        let ctx = ctx_from(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD age ON user TYPE int;
            DEFINE FIELD email ON user TYPE string;
            "#,
        );
        let table_type = ctx.build_table_type("user").unwrap();
        if let Kind::Literal(crate::types::Literal::Object(fields)) = table_type {
            assert_eq!(fields.len(), 3);
            assert_eq!(fields["name"], Kind::String);
            assert_eq!(fields["age"], Kind::Int);
            assert_eq!(fields["email"], Kind::String);
        } else {
            panic!("Expected object type");
        }
    }

    #[test]
    fn readonly_field() {
        let ctx = ctx_from(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD id ON user TYPE string READONLY;",
        );
        let field = ctx.get_field("user", "id").unwrap();
        assert!(field.readonly);
    }

    #[test]
    fn duplicate_table_def_warning() {
        let source = "DEFINE TABLE user SCHEMAFULL;\nDEFINE TABLE user SCHEMALESS;";
        let ctx = ctx_from(source);
        let diags: Vec<_> = ctx.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::DuplicateTableDef)
            .collect();
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("table `user` is already defined"));
    }

    #[test]
    fn field_on_undefined_table_strict() {
        let source = "DEFINE FIELD name ON ghost TYPE string;";
        let tree = parser::parse(source).unwrap();
        let root = tree.root_node();
        let mut ctx = Context::new();
        ctx.strict = true;
        extract_schema(&root, source, &mut ctx);
        let diags: Vec<_> = ctx.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::FieldOnUndefinedTable)
            .collect();
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("field `name` defined on table `ghost`"));
    }

    #[test]
    fn field_on_undefined_table_not_strict() {
        let source = "DEFINE FIELD name ON ghost TYPE string;";
        let ctx = ctx_from(source);
        let diags: Vec<_> = ctx.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::FieldOnUndefinedTable)
            .collect();
        assert_eq!(diags.len(), 0);
    }

    #[test]
    fn duplicate_field_def_warning() {
        let source = "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;\nDEFINE FIELD name ON user TYPE int;";
        let ctx = ctx_from(source);
        let diags: Vec<_> = ctx.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::DuplicateFieldDef)
            .collect();
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("field `name` on table `user` is already defined"));
    }

    #[test]
    fn index_on_undefined_field_warning() {
        let source = "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;\nDEFINE INDEX idx_email ON user FIELDS email;";
        let ctx = ctx_from(source);
        let diags: Vec<_> = ctx.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::IndexOnUndefinedField)
            .collect();
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("index field `email` not defined on schemafull table `user`"));
    }

    #[test]
    fn index_on_defined_field_no_warning() {
        let source = "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;\nDEFINE INDEX idx_name ON user FIELDS name;";
        let ctx = ctx_from(source);
        let diags: Vec<_> = ctx.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::IndexOnUndefinedField)
            .collect();
        assert_eq!(diags.len(), 0);
    }

    #[test]
    fn index_on_schemaless_table_no_warning() {
        let source = "DEFINE TABLE user SCHEMALESS;\nDEFINE INDEX idx_email ON user FIELDS email;";
        let ctx = ctx_from(source);
        let diags: Vec<_> = ctx.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::IndexOnUndefinedField)
            .collect();
        assert_eq!(diags.len(), 0);
    }

    #[test]
    fn record_ref_undefined_table_strict() {
        let source = "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD org ON user TYPE record<organization>;";
        let tree = parser::parse(source).unwrap();
        let root = tree.root_node();
        let mut ctx = Context::new();
        ctx.strict = true;
        extract_schema(&root, source, &mut ctx);
        let diags: Vec<_> = ctx.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::RecordRefUndefinedTable)
            .collect();
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("organization"));
    }

    #[test]
    fn record_ref_defined_table_strict_no_warning() {
        let source = "DEFINE TABLE organization SCHEMAFULL;\nDEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD org ON user TYPE record<organization>;";
        let tree = parser::parse(source).unwrap();
        let root = tree.root_node();
        let mut ctx = Context::new();
        ctx.strict = true;
        extract_schema(&root, source, &mut ctx);
        let diags: Vec<_> = ctx.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::RecordRefUndefinedTable)
            .collect();
        assert_eq!(diags.len(), 0);
    }

    #[test]
    fn record_ref_not_strict_no_warning() {
        let source = "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD org ON user TYPE record<organization>;";
        let ctx = ctx_from(source);
        let diags: Vec<_> = ctx.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::RecordRefUndefinedTable)
            .collect();
        assert_eq!(diags.len(), 0);
    }

    #[test]
    fn record_ref_nested_in_option_strict() {
        let source = "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD org ON user TYPE option<record<organization>>;";
        let tree = parser::parse(source).unwrap();
        let root = tree.root_node();
        let mut ctx = Context::new();
        ctx.strict = true;
        extract_schema(&root, source, &mut ctx);
        let diags: Vec<_> = ctx.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::RecordRefUndefinedTable)
            .collect();
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("organization"));
    }

    #[test]
    fn field_with_default_has_default() {
        let ctx = ctx_from(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD status ON user TYPE string DEFAULT 'active';",
        );
        let field = ctx.get_field("user", "status").unwrap();
        assert!(field.default.is_some(), "field with DEFAULT clause should have default set");
    }

    #[test]
    fn field_without_default_has_no_default() {
        let ctx = ctx_from(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;",
        );
        let field = ctx.get_field("user", "name").unwrap();
        assert!(field.default.is_none(), "field without DEFAULT clause should have no default");
    }

    #[test]
    fn field_with_assert_has_assert() {
        let ctx = ctx_from(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD age ON user TYPE int ASSERT $value >= 0;",
        );
        let field = ctx.get_field("user", "age").unwrap();
        assert!(field.has_assert, "field with ASSERT clause should have has_assert = true");
    }

    #[test]
    fn field_without_assert_no_assert() {
        let ctx = ctx_from(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;",
        );
        let field = ctx.get_field("user", "name").unwrap();
        assert!(!field.has_assert, "field without ASSERT clause should have has_assert = false");
    }

    #[test]
    fn field_with_default_not_required() {
        let ctx = ctx_from(
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string;
            DEFINE FIELD status ON user TYPE string DEFAULT 'active';
            "#,
        );
        let required = ctx.required_fields("user");
        assert!(required.contains(&"name".to_string()), "name should be required");
        assert!(!required.contains(&"status".to_string()), "status with DEFAULT should not be required");
    }

    #[test]
    fn nested_field_without_parent_warns() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD address.city ON user TYPE string;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::NestedFieldWithoutParent && d.message.contains("address"))
            .collect();
        assert!(!warnings.is_empty(), "Expected warning for nested field without parent, got: {:?}", result.diagnostics);
    }

    #[test]
    fn nested_field_with_parent_no_warning() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD address ON user TYPE object;
            DEFINE FIELD address.city ON user TYPE string;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::NestedFieldWithoutParent)
            .collect();
        assert!(warnings.is_empty(), "Expected no warning when parent is defined as object, got: {:?}", warnings);
    }

    #[test]
    fn nested_field_parent_wrong_type_warns() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD address ON user TYPE string;
            DEFINE FIELD address.city ON user TYPE string;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::NestedFieldWithoutParent)
            .collect();
        assert!(!warnings.is_empty(), "Expected warning when parent is not object type, got: {:?}", result.diagnostics);
    }

    #[test]
    fn deeply_nested_field_without_parent_warns() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD address ON user TYPE object;
            DEFINE FIELD address.geo.lat ON user TYPE float;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::NestedFieldWithoutParent && d.message.contains("geo"))
            .collect();
        assert!(!warnings.is_empty(), "Expected warning for missing intermediate parent, got: {:?}", result.diagnostics);
    }

    // ── Permission conflict tests ─────────────────────────────

    #[test]
    fn table_none_field_full_warns() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL PERMISSIONS NONE;
            DEFINE FIELD name ON user TYPE string PERMISSIONS FULL;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::ConflictingPermissions)
            .collect();
        assert!(!warnings.is_empty(),
            "Expected ConflictingPermissions warning for table NONE + field FULL, got: {:?}",
            result.diagnostics);
        assert!(warnings[0].message.contains("FULL"));
        assert!(warnings[0].message.contains("NONE"));
    }

    #[test]
    fn table_full_field_none_warns() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL PERMISSIONS FULL;
            DEFINE FIELD name ON user TYPE string PERMISSIONS NONE;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::ConflictingPermissions)
            .collect();
        assert!(!warnings.is_empty(),
            "Expected ConflictingPermissions warning for table FULL + field NONE, got: {:?}",
            result.diagnostics);
    }

    #[test]
    fn table_none_field_where_warns() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL PERMISSIONS NONE;
            DEFINE FIELD name ON user TYPE string
                PERMISSIONS FOR select WHERE $auth != NONE;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::ConflictingPermissions)
            .collect();
        assert!(!warnings.is_empty(),
            "Expected ConflictingPermissions warning for table NONE + field WHERE, got: {:?}",
            result.diagnostics);
    }

    #[test]
    fn table_full_field_full_no_warning() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL PERMISSIONS FULL;
            DEFINE FIELD name ON user TYPE string PERMISSIONS FULL;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::ConflictingPermissions)
            .collect();
        assert!(warnings.is_empty(),
            "Should not warn when table and field both FULL: {:?}", warnings);
    }

    #[test]
    fn table_none_field_none_no_warning() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL PERMISSIONS NONE;
            DEFINE FIELD name ON user TYPE string PERMISSIONS NONE;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::ConflictingPermissions)
            .collect();
        assert!(warnings.is_empty(),
            "Should not warn when table and field both NONE: {:?}", warnings);
    }

    #[test]
    fn no_table_permissions_no_warning() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE FIELD name ON user TYPE string PERMISSIONS FULL;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::ConflictingPermissions)
            .collect();
        assert!(warnings.is_empty(),
            "Should not warn when table has no explicit permissions: {:?}", warnings);
    }

    #[test]
    fn no_field_permissions_no_warning() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL PERMISSIONS NONE;
            DEFINE FIELD name ON user TYPE string;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::ConflictingPermissions)
            .collect();
        assert!(warnings.is_empty(),
            "Should not warn when field has no explicit permissions: {:?}", warnings);
    }

    #[test]
    fn table_permissions_stored() {
        let ctx = ctx_from("DEFINE TABLE user SCHEMAFULL PERMISSIONS NONE;");
        let table = ctx.get_table("user").unwrap();
        assert_eq!(table.permissions, Some(crate::context::PermissionLevel::None));
    }

    #[test]
    fn field_permissions_stored() {
        let ctx = ctx_from(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string PERMISSIONS FULL;",
        );
        let field = ctx.get_field("user", "name").unwrap();
        assert_eq!(field.permissions, Some(crate::context::PermissionLevel::Full));
    }

    #[test]
    fn field_custom_permissions_stored() {
        let ctx = ctx_from(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string PERMISSIONS FOR select WHERE $auth != NONE;",
        );
        let field = ctx.get_field("user", "name").unwrap();
        assert_eq!(field.permissions, Some(crate::context::PermissionLevel::Custom));
    }

    // ── REFERENCE clause tests ────────────────────────────────────

    #[test]
    fn reference_on_record_field_stored() {
        let ctx = ctx_from(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE TABLE post SCHEMAFULL;\nDEFINE FIELD author ON post TYPE record<user> REFERENCE;",
        );
        let field = ctx.get_field("post", "author").unwrap();
        assert!(field.reference, "field with REFERENCE clause should have reference = true");
    }

    #[test]
    fn reference_not_set_by_default() {
        let ctx = ctx_from(
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;",
        );
        let field = ctx.get_field("user", "name").unwrap();
        assert!(!field.reference, "field without REFERENCE clause should have reference = false");
    }

    #[test]
    fn reference_on_record_no_warning() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE post SCHEMAFULL;
            DEFINE FIELD author ON post TYPE record<user> REFERENCE;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::ReferenceOnNonRecord)
            .collect();
        assert!(warnings.is_empty(),
            "REFERENCE on record<T> field should not warn: {:?}", warnings);
    }

    #[test]
    fn reference_on_non_record_warns() {
        let result = crate::analyze(r#"
            DEFINE TABLE post SCHEMAFULL;
            DEFINE FIELD author ON post TYPE string REFERENCE;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::ReferenceOnNonRecord)
            .collect();
        assert!(!warnings.is_empty(),
            "REFERENCE on string field should warn: {:?}", result.diagnostics);
        assert!(warnings[0].message.contains("record<T>"));
    }

    #[test]
    fn reference_on_int_warns() {
        let result = crate::analyze(r#"
            DEFINE TABLE post SCHEMAFULL;
            DEFINE FIELD author ON post TYPE int REFERENCE;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::ReferenceOnNonRecord)
            .collect();
        assert!(!warnings.is_empty(),
            "REFERENCE on int field should warn: {:?}", result.diagnostics);
    }

    #[test]
    fn reference_on_option_record_no_warning() {
        let result = crate::analyze(r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE post SCHEMAFULL;
            DEFINE FIELD author ON post TYPE option<record<user>> REFERENCE;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::ReferenceOnNonRecord)
            .collect();
        assert!(warnings.is_empty(),
            "REFERENCE on option<record<T>> should not warn: {:?}", warnings);
    }

    #[test]
    fn reference_on_no_type_warns() {
        let result = crate::analyze(r#"
            DEFINE TABLE post SCHEMAFULL;
            DEFINE FIELD author ON post REFERENCE;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::ReferenceOnNonRecord)
            .collect();
        assert!(!warnings.is_empty(),
            "REFERENCE on field with no type should warn: {:?}", result.diagnostics);
    }

    #[test]
    fn reference_strict_validates_table_exists() {
        let result = crate::analyze_strict(r#"
            DEFINE TABLE post SCHEMAFULL;
            DEFINE FIELD author ON post TYPE record<nonexistent> REFERENCE;
        "#).unwrap();
        let warnings: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.code == crate::diagnostic::Code::RecordRefUndefinedTable)
            .collect();
        assert!(!warnings.is_empty(),
            "REFERENCE with undefined table should warn in strict mode: {:?}", result.diagnostics);
    }
}
