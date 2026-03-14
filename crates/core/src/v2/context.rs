/// Schema context for the v2 analyzer.
///
/// Stores table and field definitions extracted from DEFINE statements
/// in the tree-sitter CST. This replaces v1's AnalyzerContext that
/// depended on `surrealdb::sql::DefineStatement`.
use std::collections::BTreeMap;

use super::types::Type;

/// A table definition extracted from `DEFINE TABLE`.
#[derive(Debug, Clone)]
pub struct TableDef {
    pub name: String,
    pub schema_mode: SchemaMode,
    pub kind: TableKind,
    pub drop: bool,
}

/// Whether the table enforces a schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaMode {
    Schemaless,
    Schemafull,
}

/// The kind of table (normal, relation, any).
#[derive(Debug, Clone)]
pub enum TableKind {
    Any,
    Normal,
    Relation {
        from: Option<Vec<String>>,
        to: Option<Vec<String>>,
    },
}

/// A field definition extracted from `DEFINE FIELD`.
#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    pub table: String,
    pub typ: Option<Type>,
    pub default: Option<String>,
    pub readonly: bool,
    pub is_computed: bool,
}

/// Analysis context — the schema registry.
///
/// Feed it DEFINE statements (via [`Context::define_table`] / [`Context::define_field`])
/// then query it during statement analysis.
#[derive(Debug, Clone)]
pub struct Context {
    tables: BTreeMap<String, TableDef>,
    fields: BTreeMap<String, Vec<FieldDef>>,
    /// Inferred parameter types from analysis.
    inferred_params: Vec<(String, Type)>,
}

impl Context {
    pub fn new() -> Self {
        Self {
            tables: BTreeMap::new(),
            fields: BTreeMap::new(),
            inferred_params: Vec::new(),
        }
    }

    // ── Table operations ──────────────────────────────────────────

    pub fn define_table(&mut self, def: TableDef) {
        self.tables.insert(def.name.clone(), def);
    }

    pub fn get_table(&self, name: &str) -> Option<&TableDef> {
        self.tables.get(name)
    }

    pub fn has_table(&self, name: &str) -> bool {
        self.tables.contains_key(name)
    }

    // ── Field operations ──────────────────────────────────────────

    pub fn define_field(&mut self, def: FieldDef) {
        self.fields
            .entry(def.table.clone())
            .or_default()
            .push(def);
    }

    pub fn get_fields(&self, table: &str) -> &[FieldDef] {
        self.fields.get(table).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn get_field(&self, table: &str, field: &str) -> Option<&FieldDef> {
        self.get_fields(table).iter().find(|f| f.name == field)
    }

    /// Build the full object type for a table from its field definitions.
    pub fn build_table_type(&self, table: &str) -> Option<Type> {
        if !self.has_table(table) {
            return None;
        }
        let mut fields = BTreeMap::new();
        for field_def in self.get_fields(table) {
            if let Some(typ) = &field_def.typ {
                // Only include top-level fields (no dots in name).
                if !field_def.name.contains('.') {
                    fields.insert(field_def.name.clone(), typ.clone());
                }
            }
        }
        Some(Type::Object(fields))
    }

    /// Get the target table(s) of a relation for graph traversal.
    pub fn get_relation_target(&self, relation: &str, is_reverse: bool) -> Option<&[String]> {
        let table = self.get_table(relation)?;
        if let TableKind::Relation { from, to } = &table.kind {
            let targets = if is_reverse { from } else { to };
            targets.as_deref()
        } else {
            None
        }
    }

    // ── Parameter inference ───────────────────────────────────────

    pub fn add_inferred_param(&mut self, name: String, typ: Type) {
        self.inferred_params.push((name, typ));
    }

    pub fn get_inferred_param(&self, name: &str) -> Option<&Type> {
        self.inferred_params
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t)
    }

    pub fn inferred_params(&self) -> &[(String, Type)] {
        &self.inferred_params
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}
