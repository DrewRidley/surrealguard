//! Maintains analysis state and provides schema validation utilities.
//!
//! The AnalyzerContext stores:
//! - Schema definitions (tables, fields)
//! - Inferred parameter types
//! - Analysis state
//!
//! It provides methods to:
//! - Validate schema constraints
//! - Resolve types
//! - Track parameter inference
//!
//! # Examples
//!
//! ```rust
//! use surrealguard_core::prelude::*;
//! use surrealdb::sql::Kind;
//!
//! let mut ctx = AnalyzerContext::new();
//!
//! // Add inferred parameter directly
//! ctx.add_inferred_param("$user", Kind::String);
//!
//! // Get inferred types
//! let param_types = ctx.get_all_inferred_params();
//! assert_eq!(param_types.len(), 1);
//! ```
use std::collections::BTreeMap;
use surrealdb::sql::statements::{DefineFieldStatement, DefineTableStatement};
use surrealdb::sql::{statements::DefineStatement, Geometry, Kind, Table, Value};
use surrealdb::sql::{Idiom, Literal, Part, TableType};

use super::error::{AnalyzerError, AnalyzerResult};

#[derive(Clone)]
pub struct AnalyzerContext {
    definitions: Vec<DefineStatement>,
    /// Parameters whose types are inferred based on usage or positioning.
    ///
    /// In certain contexts, particularly UPDATE or CREATE,
    /// It is possible to infer the required type of a parameter.
    /// This has to be bubbled up to the codegen for processing.
    inferred_params: Vec<(String, Kind)>,
    /// Table name for the current scope user.
    auth: Option<String>,

    // A list of identifiers and their corresponding
    // justifications for alterations to the original 'Kind'.
    permissions: BTreeMap<String, String>,
}

impl AnalyzerContext {
    pub fn new() -> Self {
        Self {
            definitions: Vec::new(),
            inferred_params: Vec::new(),
            auth: None,
            permissions: BTreeMap::new(),
        }
    }

    pub fn auth(&self) -> Option<&str> {
        self.auth.as_deref()
    }

    /// Registers a permission for a field path.
    ///
    /// This function allows you to associate a permission with a specific field path.
    /// The field path is a string representing the path to the field, and the permission
    /// is a string representing the required permission for accessing the field.
    ///
    /// Example usage:
    ///
    /// ```
    /// use surrealguard_core::prelude::AnalyzerContext;
    /// let mut context = AnalyzerContext::new();
    /// context.register_permission("users.email", "read");
    /// ```
    pub fn register_permission(&mut self, field_path: &str, permission: &str) {
        self.permissions
            .insert(field_path.to_string(), permission.to_string());
    }

    pub fn add_inferred_param(&mut self, name: &str, kind: Kind) {
        self.inferred_params.push((name.to_string(), kind));
    }

    pub fn get_inferred_param(&self, name: &str) -> Option<&Kind> {
        self.inferred_params
            .iter()
            .find(|(param_name, _)| param_name == name)
            .map(|(_, kind)| kind)
    }

    pub fn get_all_inferred_params(&self) -> &[(String, Kind)] {
        &self.inferred_params
    }

    pub fn infer_param_from_field(
        &mut self,
        table: &str,
        field: &Idiom,
        param: &str,
    ) -> AnalyzerResult<()> {
        if let Some(DefineStatement::Field(field_def)) = self.find_field_definition(table, field) {
            if let Some(kind) = field_def.kind.clone() {
                self.add_inferred_param(param, kind);
                Ok(())
            } else {
                Err(AnalyzerError::schema_violation(
                    "Field type not defined",
                    Some(table),
                    Some(field.to_string()),
                ))
            }
        } else {
            Err(AnalyzerError::field_not_found(field.to_string(), table))
        }
    }

    pub fn infer_param_from_table(&mut self, table: &str, param: &str) -> AnalyzerResult<()> {
        let table_type = self.build_full_table_type(table)?;
        self.add_inferred_param(param, table_type);
        Ok(())
    }

    /// Gets the target table of a relation
    pub fn get_relation_target(&self, relation_table: &str, is_reverse: bool) -> Option<String> {
        if let Some(DefineStatement::Table(table_def)) = self.find_table_definition(relation_table)
        {
            if let TableType::Relation(rel) = &table_def.kind {
                // For reverse traversals (<-), return "from"; for forward (->), return "to"
                if let Some(Kind::Record(tables)) = if is_reverse {
                    rel.from.as_ref()
                } else {
                    rel.to.as_ref()
                } {
                    // Get the first table name from the record type
                    tables.first().map(|t| t.0.clone())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn build_full_table_type(&self, table_name: &str) -> AnalyzerResult<Kind> {
        let mut field_types = BTreeMap::new();
        for field_def in self.get_field_definitions(table_name) {
            if let Some(kind) = field_def.kind.clone() {
                field_types.insert(field_def.name.to_string(), kind);
            }
        }
        Ok(Kind::Literal(Literal::Object(field_types)))
    }

    /// Finds a relation definition (i.e. a table whose TableType is Relation)
    /// matching the given relation idiom.
    pub fn find_relation_definition(
        &self,
        relation_idiom: &Idiom,
    ) -> Option<&DefineTableStatement> {
        self.definitions.iter().find_map(|def| {
            if let DefineStatement::Table(table_def) = def {
                // Compare the table’s name with the given relation idiom.
                // Here we compare the normalized strings.
                if table_def
                    .name
                    .to_string()
                    .eq_ignore_ascii_case(&relation_idiom.to_string())
                    && matches!(table_def.kind, TableType::Relation(_))
                {
                    Some(table_def)
                } else {
                    None
                }
            } else {
                None
            }
        })
    }

    pub fn find_table_definition(&self, table_name: &str) -> Option<&DefineStatement> {
        self.definitions.iter().find(|def| {
            if let DefineStatement::Table(table_def) = def {
                table_def.name.0 == table_name
            } else {
                false
            }
        })
    }

    pub fn get_field_definitions(&self, table_name: &str) -> Vec<&DefineFieldStatement> {
        self.definitions
            .iter()
            .filter_map(|def| {
                if let DefineStatement::Field(field_def) = def {
                    if field_def.what.0 == table_name {
                        Some(field_def)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn find_field_definition(
        &self,
        table_name: &str,
        field_idiom: &Idiom,
    ) -> Option<&DefineStatement> {
        // Try exact match first
        let exact_match = self.definitions.iter().find(|def| {
            if let DefineStatement::Field(field_def) = def {
                field_def.what.0 == table_name && &field_def.name == field_idiom
            } else {
                false
            }
        });

        if exact_match.is_some() {
            return exact_match;
        }

        // If no exact match, try parent paths
        if field_idiom.0.len() > 1 {
            let parent_parts: Vec<Part> = field_idiom.0[..field_idiom.0.len() - 1]
                .iter()
                .map(|p| Part::from(p.to_string()))
                .collect();

            let parent_idiom = Idiom::from(parent_parts);
            self.find_field_definition(table_name, &parent_idiom)
        } else {
            None
        }
    }

    pub fn append_definition(&mut self, definition: DefineStatement) {
        self.definitions.push(definition);
    }
    pub fn resolve(&self, value: &Value) -> AnalyzerResult<Kind> {
        Ok(match value {
            Value::None => Kind::Null,
            Value::Null => Kind::Null,
            Value::Bool(_) => Kind::Bool,
            Value::Number(_) => Kind::Number,
            Value::Strand(_) => Kind::String,
            Value::Duration(_) => Kind::Duration,
            Value::Datetime(_) => Kind::Datetime,
            Value::Uuid(_) => Kind::Uuid,
            Value::Array(array) => {
                if array.is_empty() {
                    Kind::Array(Box::new(Kind::Any), None)
                } else {
                    Kind::Array(Box::new(self.resolve(&array[0])?), None)
                }
            }
            Value::Object(_) => Kind::Object,
            Value::Geometry(geometry) => match geometry {
                Geometry::Point(_) => Kind::Geometry(vec!["point".to_string()]),
                Geometry::Line(_) => Kind::Geometry(vec!["line".to_string()]),
                Geometry::Polygon(_) => Kind::Geometry(vec!["polygon".to_string()]),
                Geometry::MultiPoint(_) => Kind::Geometry(vec!["multipoint".to_string()]),
                Geometry::MultiLine(_) => Kind::Geometry(vec!["multiline".to_string()]),
                Geometry::MultiPolygon(_) => Kind::Geometry(vec!["multipolygon".to_string()]),
                Geometry::Collection(_) => Kind::Geometry(vec!["collection".to_string()]),
                other => {
                    return Err(AnalyzerError::Unimplemented(format!(
                        "Geometry variant not supported: {:?}",
                        other
                    )))
                }
            },
            Value::Bytes(_) => Kind::Bytes,
            Value::Thing(thing) => Kind::Record(vec![Table::from(thing.tb.clone())]),
            Value::Table(table) => Kind::Record(vec![table.clone()]),
            Value::Range(_) => Kind::Range,
            Value::Function(_) => Kind::Function(None, None),
            Value::Model(_) => Kind::Object,
            Value::Mock(_)
            | Value::Param(_)
            | Value::Idiom(_)
            | Value::Regex(_)
            | Value::Cast(_)
            | Value::Block(_)
            | Value::Edges(_)
            | Value::Future(_)
            | Value::Constant(_)
            | Value::Subquery(_)
            | Value::Expression(_)
            | Value::Query(_)
            | Value::Closure(_) => Kind::Any,
            _ => {
                return Err(AnalyzerError::Unimplemented(
                    "Unexpected Value variant".into(),
                ))
            }
        })
    }
}
