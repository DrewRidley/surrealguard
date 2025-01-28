//! Management of database index definitions and metadata

use std::collections::HashMap;
use surrealdb::sql::{Ident, Table};

/// Contains information about database indexes
#[derive(Debug, Default, Clone)]
pub struct IndexesContext {
    /// Map of index names to their definitions
    indexes: HashMap<String, IndexDefinition>,
}

#[derive(Debug, Clone)]
pub struct IndexDefinition {
    pub table: Table,
    pub fields: Vec<Ident>,
    pub index_type: IndexType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IndexType {
    Unique,
    Search,
    Vector,
    Custom(String),
}

impl IndexesContext {
    /// Adds a new index definition
    pub fn add_index(&mut self, name: String, definition: IndexDefinition) {
        self.indexes.insert(name, definition);
    }

    pub fn remove_index(&mut self, name: &str) {
           self.indexes.remove(name);
    }

    /// Returns all indexes for a specific table
    pub fn get_table_indexes(&self, table: &str) -> Vec<&IndexDefinition> {
        self.indexes.values()
            .filter(|def| def.table.0 == table)
            .collect()
    }
}
