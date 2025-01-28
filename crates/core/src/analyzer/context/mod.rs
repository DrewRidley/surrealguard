//! The analysis context system for tracking type information, schemas, and
//! other metadata during query analysis.
//!
//! The [`AnalyzerContext`] is divided into multiple sub-contexts that handle
//! different aspects of the analysis environment.

pub mod schema;
pub mod params;
pub mod functions;
pub mod indexes;
pub mod events;
pub mod tokens;

use std::{collections::HashMap, sync::Arc};
use surrealdb::sql::{Ident, Number, Permissions, Value};
use crate::analyzer::model::{Type, TypeKind, TypeMetadata};


/// Primary container for all analysis context information.
///
/// Uses composition to separate different categories of context information
/// while maintaining a single analysis interface. The context is designed
/// to be extended with additional sub-contexts as needed.
#[derive(Debug, Default)]
pub struct AnalyzerContext {
    /// Schema-related context (tables, indexes, analyzers)
    pub schema: schema::SchemaContext,
    /// Query parameters and variables
    pub params: params::ParamsContext,
    /// Function definitions and signatures
    pub functions: functions::FunctionsContext,
    /// Database indexes information
    pub indexes: indexes::IndexesContext,
    /// Event handlers and triggers
    pub events: events::EventsContext,
    /// Authentication tokens and permissions
    pub tokens: tokens::TokensContext,
}

impl AnalyzerContext {
    /// Creates a new analysis context with default sub-contexts
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a child context for analyzing subqueries
    ///
    /// Notably, this provides a new params context.
    pub fn create_child(&self) -> Self {
        Self {
            schema: self.schema.clone(),
            params: params::ParamsContext::new(),
            functions: self.functions.clone(),
            indexes: self.indexes.clone(),
            events: self.events.clone(),
            tokens: self.tokens.clone(),
        }
    }


    pub fn resolve(&self, value: &Value) -> Type {
        match value {
            Value::None | Value::Null => Type::null(),
            Value::Bool(_) => Type::bool(),
            Value::Number(n) => match n {
                Number::Int(_) => Type::int(),
                Number::Float(_) => Type::float(),
                Number::Decimal(_) => Type::decimal(),
                _ => Type::unknown(),
            },
            Value::Strand(_) => Type::string(),
            Value::Duration(_) => Type::duration(),
            Value::Datetime(_) => Type::datetime(),
            Value::Uuid(_) => Type::uuid(),
            Value::Array(arr) => {
                let element_types: Vec<Type> = arr.iter().map(|v| self.resolve(v)).collect();
                // Check if all elements are the same type
                if let Some(t) = element_types.first() {
                    if element_types.iter().all(|et| et == t) {
                        Type::array(t.clone())
                    } else {
                        Type::any_array() // or union of all types
                    }
                } else {
                    Type::array(Type::any())
                }
            }
            Value::Object(obj) => {
                let fields: HashMap<String, Type> = obj.iter().map(|(k, v)| (k.clone(), self.resolve(v))).collect();
                Type::object(fields)
            }
            Value::Thing(thing) => {
                self.schema.get_table_type(&thing.tb).cloned().unwrap_or(Type::unknown())
            }
            Value::Param(param) => {
                self.params.get_parameter_type(&param.0).cloned().unwrap_or(Type::unknown())
            }
            Value::Function(func) => {
                if let Some(sig) = self.functions.get_function(func.name().unwrap()) {
                    sig.return_type.clone()
                } else {
                    Type::unknown()
                }
            },
            Value::Table(table) => {
                        // Table.0 contains the table name as a string
                        self.schema.get_table_type(&table.0).cloned().unwrap_or(Type::unknown())
            },
            // Handle other cases like Subquery, Cast, etc.
            _ => Type::unknown(),
        }
    }
}
