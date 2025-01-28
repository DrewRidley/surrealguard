//! Schema-related context including tables, columns, and schema validation rules

use std::collections::HashMap;
use crate::analyzer::model::Type;

/// Manages schema information about database tables and their structure
#[derive(Debug, Default, Clone)]
pub struct SchemaContext {
    /// Map of table names to their type definitions
    pub tables: HashMap<String, Type>,
    /// Map of analyzer names to their definitions
    pub analyzers: HashMap<String, Type>,
    /// Schema validation rules and constraints
    pub validation_rules: HashMap<String, SchemaValidationRule>,
}

/// Validation rule for schema constraints
#[derive(Debug, Clone)]
pub struct SchemaValidationRule {
    pub description: String,
    pub condition: String,
    pub severity: ValidationSeverity,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ValidationSeverity {
    Error,
    Warning,
    Info,
}

impl SchemaContext {
    /// Adds a table definition to the schema
    pub fn add_table(&mut self, name: String, table_type: Type) {
        self.tables.insert(name, table_type);
    }

    /// Retrieves a table type by name, if exists
    pub fn get_table_type(&self, name: &str) -> Option<&Type> {
        self.tables.get(name)
    }
}
