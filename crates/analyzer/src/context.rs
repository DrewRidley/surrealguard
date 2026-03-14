/// Schema registry and analysis context.
///
/// Stores table/field/index/function definitions extracted from DEFINE statements.
/// Combined with the scope system for full analysis state.
use std::collections::BTreeMap;

use crate::diagnostic::Diagnostic;
use crate::scope::Scope;
use crate::span::Span;
use crate::types::{Kind, Literal};

// ── Schema Definitions ──────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TableDef {
    pub name: String,
    pub span: Span,
    pub schema_mode: SchemaMode,
    pub kind: TableKind,
    pub drop: bool,
    pub permissions: Option<PermissionLevel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaMode {
    Schemaless,
    Schemafull,
}

/// Represents the permission level for a table or field.
///
/// In SurrealDB, permissions can be:
/// - `FULL` — unrestricted access
/// - `NONE` — no access
/// - `WHERE ...` — conditional access (treated as "Custom" here)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionLevel {
    Full,
    None,
    Custom,
}

#[derive(Debug, Clone)]
pub enum TableKind {
    Any,
    Normal,
    Relation {
        from: Option<Vec<String>>,
        to: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    pub table: String,
    pub span: Span,
    pub typ: Option<Kind>,
    pub default: Option<String>,
    pub readonly: bool,
    pub is_computed: bool,
    pub flexible: bool,
    pub has_assert: bool,
    pub reference: bool,
    pub permissions: Option<PermissionLevel>,
}

#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub name: String,
    pub span: Span,
    pub params: Vec<(String, Kind)>,
    pub return_type: Option<Kind>,
}

#[derive(Debug, Clone)]
pub struct IndexDef {
    pub name: String,
    pub table: String,
    pub span: Span,
    pub fields: Vec<String>,
    pub unique: bool,
}

// ── Analysis Context ─────────────────────────────────────────

/// Full analysis context combining schema registry, scope, and config.
#[derive(Debug, Clone)]
pub struct Context {
    // Schema
    tables: BTreeMap<String, TableDef>,
    fields: BTreeMap<String, Vec<FieldDef>>,
    functions: BTreeMap<String, FunctionDef>,
    indexes: Vec<IndexDef>,

    // Analysis state
    pub scope: Scope,

    // Config
    pub strict: bool,

    // Collected diagnostics
    pub diagnostics: Vec<Diagnostic>,

    // Inferred parameter types (for codegen)
    inferred_params: Vec<(String, Kind),>,

    // Transaction tracking
    pub transaction_depth: usize,
}

impl Context {
    pub fn new() -> Self {
        Self {
            tables: BTreeMap::new(),
            fields: BTreeMap::new(),
            functions: BTreeMap::new(),
            indexes: Vec::new(),
            scope: Scope::new(),
            strict: false,
            diagnostics: Vec::new(),
            inferred_params: Vec::new(),
            transaction_depth: 0,
        }
    }

    /// Create a context with strict mode enabled (errors on ambiguous types).
    pub fn strict() -> Self {
        let mut ctx = Self::new();
        ctx.strict = true;
        ctx
    }

    // ── Diagnostics ──────────────────────────────────────────

    pub fn emit(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.is_error())
    }

    pub fn take_diagnostics(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    // ── Table operations ─────────────────────────────────────

    pub fn define_table(&mut self, def: TableDef) {
        self.tables.insert(def.name.clone(), def);
    }

    pub fn get_table(&self, name: &str) -> Option<&TableDef> {
        self.tables.get(name)
    }

    pub fn get_table_mut(&mut self, name: &str) -> Option<&mut TableDef> {
        self.tables.get_mut(name)
    }

    pub fn has_table(&self, name: &str) -> bool {
        self.tables.contains_key(name)
    }

    pub fn table_names(&self) -> impl Iterator<Item = &str> {
        self.tables.keys().map(|s| s.as_str())
    }

    // ── Field operations ─────────────────────────────────────

    pub fn define_field(&mut self, def: FieldDef) {
        self.fields
            .entry(def.table.clone())
            .or_default()
            .push(def);
    }

    pub fn get_fields(&self, table: &str) -> &[FieldDef] {
        self.fields.get(table).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get all table names that have field definitions.
    pub fn field_table_names(&self) -> Vec<String> {
        self.fields.keys().cloned().collect()
    }

    pub fn get_field(&self, table: &str, field: &str) -> Option<&FieldDef> {
        self.get_fields(table).iter().find(|f| f.name == field)
    }

    /// Get a field, searching parent paths if not found exactly.
    /// e.g., looking for "address.city" falls back to "address" if no exact match.
    pub fn get_field_or_parent(&self, table: &str, field: &str) -> Option<&FieldDef> {
        if let Some(f) = self.get_field(table, field) {
            return Some(f);
        }
        // Try parent paths
        if let Some(dot_pos) = field.rfind('.') {
            let parent = &field[..dot_pos];
            self.get_field_or_parent(table, parent)
        } else {
            None
        }
    }

    /// Get all top-level field names for a table.
    pub fn field_names(&self, table: &str) -> Vec<&str> {
        self.get_fields(table)
            .iter()
            .filter(|f| !f.name.contains('.'))
            .map(|f| f.name.as_str())
            .collect()
    }

    /// Build the full object type for a table from its field definitions.
    pub fn build_table_type(&self, table: &str) -> Option<Kind> {
        self.build_table_type_filtered(table, &[], &[])
    }

    /// Build the full object type for a table, omitting specified fields.
    pub fn build_table_type_with_omit(&self, table: &str, omit: &[String]) -> Option<Kind> {
        self.build_table_type_filtered(table, omit, &[])
    }

    /// Build an object type for a table, optionally omitting or restricting to specific fields.
    /// If `restrict` is non-empty, only those fields are included.
    fn build_table_type_filtered(
        &self,
        table: &str,
        omit: &[String],
        restrict: &[String],
    ) -> Option<Kind> {
        if !self.has_table(table) {
            return None;
        }
        let mut obj = BTreeMap::new();
        for field in self.get_fields(table) {
            if let Some(typ) = &field.typ {
                // Only top-level fields
                if !field.name.contains('.') {
                    // Skip omitted fields
                    if omit.iter().any(|o| o == &field.name) {
                        continue;
                    }
                    // If restricting, only include matching fields
                    if !restrict.is_empty() && !restrict.iter().any(|r| r == &field.name) {
                        continue;
                    }
                    obj.insert(field.name.clone(), typ.clone());
                }
            }
        }
        Some(Kind::Literal(Literal::Object(obj)))
    }

    /// Expand record links in a type by replacing `record<T>` with the full table schema.
    /// Used by FETCH clause processing.
    pub fn expand_record_links(&self, kind: &Kind, fetch_fields: &[String]) -> Kind {
        match kind {
            Kind::Literal(Literal::Object(fields)) => {
                let mut new_fields = BTreeMap::new();
                for (name, typ) in fields {
                    if fetch_fields.iter().any(|f| f == name) {
                        // This field is being FETCHed — expand record links
                        new_fields.insert(name.clone(), self.expand_record_type(typ));
                    } else {
                        new_fields.insert(name.clone(), typ.clone());
                    }
                }
                Kind::Literal(Literal::Object(new_fields))
            }
            Kind::Array(inner, len) => {
                Kind::Array(Box::new(self.expand_record_links(inner, fetch_fields)), *len)
            }
            _ => kind.clone(),
        }
    }

    /// Expand a record type to its full table schema.
    fn expand_record_type(&self, kind: &Kind) -> Kind {
        match kind {
            Kind::Record(tables) if tables.len() == 1 => {
                let table_name = tables[0].to_string();
                self.build_table_type(&table_name).unwrap_or(kind.clone())
            }
            Kind::Array(inner, len) => {
                Kind::Array(Box::new(self.expand_record_type(inner)), *len)
            }
            Kind::Option(inner) => {
                Kind::Option(Box::new(self.expand_record_type(inner)))
            }
            _ => kind.clone(),
        }
    }

    // ── Function operations ──────────────────────────────────

    pub fn define_function(&mut self, def: FunctionDef) {
        self.functions.insert(def.name.clone(), def);
    }

    pub fn get_function(&self, name: &str) -> Option<&FunctionDef> {
        self.functions.get(name)
    }

    // ── Index operations ─────────────────────────────────────

    pub fn define_index(&mut self, def: IndexDef) {
        self.indexes.push(def);
    }

    pub fn has_index(&self, table: &str, name: &str) -> bool {
        self.indexes
            .iter()
            .any(|idx| idx.table == table && idx.name == name)
    }

    // ── Relation operations ──────────────────────────────────

    pub fn get_relation_target(&self, relation: &str, is_reverse: bool) -> Option<&[String]> {
        let table = self.get_table(relation)?;
        if let TableKind::Relation { from, to } = &table.kind {
            let targets = if is_reverse { from } else { to };
            targets.as_deref()
        } else {
            None
        }
    }

    pub fn is_schemafull(&self, table: &str) -> bool {
        self.get_table(table)
            .map(|t| t.schema_mode == SchemaMode::Schemafull)
            .unwrap_or(false)
    }

    /// Returns field names that are required for a CREATE/INSERT on the given table.
    ///
    /// A field is required if:
    /// - The table is SCHEMAFULL
    /// - The field is a top-level field (no dots)
    /// - The field is NOT `id` (auto-generated)
    /// - The field type is NOT `Option(...)`, `Null`, or `Any`
    /// - The field is NOT computed
    /// - The field has no default value
    pub fn required_fields(&self, table: &str) -> Vec<String> {
        if !self.is_schemafull(table) {
            return Vec::new();
        }
        self.get_fields(table)
            .iter()
            .filter(|f| {
                // Only top-level fields
                if f.name.contains('.') {
                    return false;
                }
                // Skip `id` — auto-generated
                if f.name == "id" {
                    return false;
                }
                // Skip computed fields
                if f.is_computed {
                    return false;
                }
                // Skip fields with a default value
                if f.default.is_some() {
                    return false;
                }
                // Skip optional/nullable/any types
                match &f.typ {
                    Some(Kind::Option(_)) | Some(Kind::Null) | Some(Kind::Any) | None => false,
                    _ => true,
                }
            })
            .map(|f| f.name.clone())
            .collect()
    }

    pub fn is_relation(&self, table: &str) -> bool {
        self.get_table(table)
            .map(|t| matches!(t.kind, TableKind::Relation { .. }))
            .unwrap_or(false)
    }

    // ── Parameter inference ──────────────────────────────────

    pub fn add_inferred_param(&mut self, name: String, typ: Kind) {
        self.inferred_params.push((name, typ));
    }

    pub fn inferred_params(&self) -> &[(String, Kind)] {
        &self.inferred_params
    }

    // ── Utility ──────────────────────────────────────────────

    /// Resolve a field path type against a table.
    /// e.g., "address.city" on table "user" where address is object<{city: string}>
    pub fn resolve_field_path(&self, table: &str, path: &[&str]) -> Option<Kind> {
        if path.is_empty() {
            return None;
        }

        let field = self.get_field(table, path[0])?;
        let mut current_type = field.typ.clone()?;

        for &segment in &path[1..] {
            match &current_type {
                Kind::Literal(Literal::Object(fields)) => {
                    current_type = fields.get(segment)?.clone();
                }
                _ => return None,
            }
        }

        Some(current_type)
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx() -> Context {
        let mut ctx = Context::new();
        ctx.define_table(TableDef {
            name: "user".into(),
            span: Span::new(0, 0),
            schema_mode: SchemaMode::Schemafull,
            kind: TableKind::Normal,
            drop: false,
            permissions: None,
        });
        ctx.define_field(FieldDef {
            name: "name".into(),
            table: "user".into(),
            span: Span::new(0, 0),
            typ: Some(Kind::String),
            default: None,
            readonly: false,
            is_computed: false,
            flexible: false,
            has_assert: false,
            reference: false,
            permissions: None,
        });
        ctx.define_field(FieldDef {
            name: "age".into(),
            table: "user".into(),
            span: Span::new(0, 0),
            typ: Some(Kind::Int),
            default: None,
            readonly: false,
            is_computed: false,
            flexible: false,
            has_assert: false,
            reference: false,
            permissions: None,
        });
        ctx
    }

    #[test]
    fn build_table_type() {
        let ctx = test_ctx();
        let typ = ctx.build_table_type("user").unwrap();
        if let Kind::Literal(Literal::Object(fields)) = typ {
            assert_eq!(fields["name"], Kind::String);
            assert_eq!(fields["age"], Kind::Int);
        } else {
            panic!("Expected object type");
        }
    }

    #[test]
    fn field_names() {
        let ctx = test_ctx();
        let names = ctx.field_names("user");
        assert!(names.contains(&"name"));
        assert!(names.contains(&"age"));
    }

    #[test]
    fn strict_mode() {
        let ctx = Context::strict();
        assert!(ctx.strict);
    }
}
