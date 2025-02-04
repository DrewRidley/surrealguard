//! Analysis of SELECT statements in SurrealQL.
//!
//! This module handles type checking and validation of SELECT queries, including:
//! - Field resolution and type inference
//! - Table existence verification
//! - Support for field aliases, wildcards, omit, fetch, VALUE, and ONLY clauses
//! - Nested field access and type transformations (e.g. fetching record links)
//!
//! The FETCH clause is applied last. When a record link is encountered and the FETCH clause
//! provides a table name matching the link’s target, the record link is replaced with the full
//! table schema.
//!
//! For SELECT VALUE statements, the analyzer expects exactly one field expression and returns
//! an array (wrapped in a Literal) whose element type is the type of that expression (looked up
//! from the schema).
//!
//! For normal SELECT queries, the return type is the record type (an object), without any array wrapping.
//! (In particular, ONLY queries return a single object.)

use std::collections::BTreeMap;
use surrealdb::sql::{
    statements::{DefineStatement, SelectStatement},
    Field, Idiom, Idioms, Kind, Literal, Part, Table, Value, Fetch,
};
use crate::analyzer::{
    context::AnalyzerContext,
    error::{AnalyzerError, AnalyzerResult},
};

pub fn analyze_select(context: &AnalyzerContext, stmt: &SelectStatement) -> AnalyzerResult<Kind> {
    // Determine the target table name from the "what" clause.
    // For ONLY queries the table reference might be a Thing (e.g. "person:tobie") so we support that.
    let table_value = stmt.what.0.first().ok_or(AnalyzerError::UnexpectedSyntax)?;
    let raw_table_name = match table_value {
        Value::Table(t) => t.0.clone(),
        Value::Thing(thing) => thing.tb.clone(),
        _ => return Err(AnalyzerError::UnexpectedSyntax),
    };
    let table_name = if raw_table_name.contains(':') {
        // In case the table name itself has a colon (unlikely), split on it.
        raw_table_name.split(':').next().unwrap().to_string()
    } else {
        raw_table_name
    };

    // Ensure that the table exists.
    if context.find_table_definition(&table_name).is_none() {
        return Err(AnalyzerError::TableNotFound(table_name));
    }

    // Check if the select is a VALUE select.
    // The AST for fields is (Vec<Field>, bool) where the bool indicates VALUE mode.
    let is_value_select = stmt.expr.1;

    if is_value_select {
        // For SELECT VALUE, exactly one expression must be present.
        if stmt.expr.0.len() != 1 {
            return Err(AnalyzerError::UnexpectedSyntax);
        }
        match &stmt.expr.0[0] {
            Field::Single { expr, .. } => {
                // Expect an idiom referencing a field.
                let field_idiom = match expr {
                    Value::Idiom(idiom) => idiom,
                    _ => return Err(AnalyzerError::UnexpectedSyntax),
                };
                // Lookup the field definition on the table.
                if let Some(DefineStatement::Field(field_def)) =
                    context.find_field_definition(&table_name, field_idiom)
                {
                    let mut resolved = field_def.kind.clone().unwrap_or(Kind::Any);
                    // Apply FETCH transformation if present.
                    if let Some(fetches) = stmt.fetch.as_ref() {
                        let fetch_chain = fetches_to_chain(fetches);
                        resolved = resolved.resolve_fetch(&fetch_chain, context);
                    }
                    // SELECT VALUE returns an array of the resolved type.
                    return Ok(Kind::Literal(Literal::Array(vec![resolved])));
                } else {
                    return Err(AnalyzerError::field_not_found(field_idiom.to_string(), &table_name));
                }
            }
            _ => return Err(AnalyzerError::UnexpectedSyntax),
        }
    }

    // For normal SELECT queries, build the record object.
    let base_kind = if stmt.expr.0.is_empty() || stmt.expr.0.iter().any(|f| matches!(f, Field::All)) {
        build_full_table_type(context, &table_name, stmt.omit.as_ref())?
    } else {
        let mut field_types = BTreeMap::new();
        for field in &stmt.expr.0 {
            match field {
                Field::Single { expr, alias } => {
                    let field_idiom = match expr {
                        Value::Idiom(idiom) => idiom,
                        _ => return Err(AnalyzerError::UnexpectedSyntax),
                    };
                    if should_omit_field(field_idiom, stmt.omit.as_ref()) {
                        continue;
                    }
                    if let Some(DefineStatement::Field(field_def)) =
                        context.find_field_definition(&table_name, field_idiom)
                    {
                        let output_name = if let Some(alias_name) = alias {
                            alias_name.to_string()
                        } else {
                            field_idiom.to_string()
                        };
                        if let Some(kind) = field_def.kind.clone() {
                            field_types.insert(output_name, kind);
                        } else {
                            return Err(AnalyzerError::schema_violation(
                                "Field type not defined",
                                Some(&table_name),
                                Some(&field_idiom.to_string()),
                            ));
                        }
                    } else {
                        return Err(AnalyzerError::field_not_found(
                            field_idiom.to_string(),
                            &table_name,
                        ));
                    }
                }
                _ => return Err(AnalyzerError::UnexpectedSyntax),
            }
        }
        Kind::Literal(Literal::Object(field_types))
    };

    // Apply FETCH transformation if present.
    let transformed_kind = if let Some(fetches) = stmt.fetch.as_ref() {
        let fetch_chain = fetches_to_chain(fetches);
        base_kind.resolve_fetch(&fetch_chain, context)
    } else {
        base_kind
    };

    // For normal SELECT queries (non-VALUE), the ONLY flag simply means we return the object directly.
    Ok(transformed_kind)
}

/// Returns true if the given field (represented by an Idiom) appears in the omit clause.
fn should_omit_field(field_path: &Idiom, omit_idioms: Option<&Idioms>) -> bool {
    if let Some(idioms) = omit_idioms {
        for omit_idiom in idioms.0.iter() {
            if field_path == omit_idiom {
                return true;
            }
        }
    }
    false
}

/// Builds the full type for a table by collecting its field definitions and applying omit, if any.
fn build_full_table_type(
    context: &AnalyzerContext,
    table_name: &str,
    omit_idioms: Option<&Idioms>,
) -> AnalyzerResult<Kind> {
    let mut field_types = BTreeMap::new();
    for field_def in context.get_field_definitions(table_name) {
        if let Some(kind) = field_def.kind.clone() {
            field_types.insert(field_def.name.to_string(), kind);
        }
    }
    if let Some(idioms) = omit_idioms {
        for idiom in idioms.0.iter() {
            remove_nested_field(&mut field_types, &idiom.0);
        }
    }
    Ok(Kind::Literal(Literal::Object(field_types)))
}

/// Recursively removes a field (or nested field) from an object.
fn remove_nested_field(obj: &mut BTreeMap<String, Kind>, path: &[Part]) {
    if path.is_empty() {
        return;
    }
    let omit_key = normalized_part(&path[0]);
    if path.len() == 1 {
        let keys: Vec<String> = obj.keys().cloned().collect();
        for key in keys {
            if key.trim().to_lowercase() == omit_key {
                obj.remove(&key);
            }
        }
    } else {
        let keys: Vec<String> = obj.keys().cloned().collect();
        for key in keys {
            if key.trim().to_lowercase() == omit_key {
                if let Some(Kind::Literal(Literal::Object(nested_obj))) = obj.get_mut(&key) {
                    remove_nested_field(nested_obj, &path[1..]);
                }
            }
        }
    }
}

/// Normalizes a single Part to a lowercase string.
fn normalized_part(part: &Part) -> String {
    match part {
        Part::Field(ident) => ident.trim().to_lowercase(),
        _ => part.to_string().trim().to_lowercase(),
    }
}

/// Converts a vector of Fetch items into a chain of strings representing the fetch path.
fn fetches_to_chain(fetches: &Vec<Fetch>) -> Vec<String> {
    fetches
        .iter()
        .map(|f| f.0.to_string().trim().to_lowercase())
        .collect()
}

/// Extension trait on Kind to allow resolving a fetch chain.
pub trait KindFetchExt {
    /// Resolves the fetch chain on self.
    /// For any record link encountered whose target table matches the first element
    /// of the fetch chain, the record link is replaced with the full table type.
    /// The chain is then applied recursively.
    fn resolve_fetch(&self, fetch_chain: &[String], context: &AnalyzerContext) -> Self;
}

impl KindFetchExt for Kind {
    fn resolve_fetch(&self, fetch_chain: &[String], context: &AnalyzerContext) -> Self {
        if fetch_chain.is_empty() {
            return self.clone();
        }
        match self {
            // For a record link, check whether any table in the link matches the fetch segment.
            Kind::Record(tables) => {
                let target = fetch_chain[0].trim().to_lowercase();
                if let Some(matching_table) =
                    tables.iter().find(|t| t.0.trim().to_lowercase() == target)
                {
                    if let Ok(full_type) = build_full_table_type(context, &matching_table.0, None) {
                        return full_type.resolve_fetch(&fetch_chain[1..], context);
                    }
                }
                self.clone()
            }
            // For an object literal, apply fetch resolution on each field.
            Kind::Literal(Literal::Object(map)) => {
                let mut new_map = BTreeMap::new();
                for (k, v) in map {
                    new_map.insert(k.clone(), v.resolve_fetch(fetch_chain, context));
                }
                Kind::Literal(Literal::Object(new_map))
            }
            // For an array, wrap the resolved inner type in a Literal::Array.
            Kind::Array(inner, _len) => {
                let new_inner = inner.resolve_fetch(fetch_chain, context);
                Kind::Literal(Literal::Array(vec![new_inner]))
            }
            // For union types, apply on each branch.
            Kind::Either(kinds) => {
                let new_kinds = kinds
                    .iter()
                    .map(|k| k.resolve_fetch(fetch_chain, context))
                    .collect();
                Kind::Either(new_kinds)
            }
            _ => self.clone(),
        }
    }
}

/// Applies the FETCH transformation using the given fetch chain.
fn apply_fetch(context: &AnalyzerContext, kind: Kind, fetch_chain: &[String]) -> Kind {
    kind.resolve_fetch(fetch_chain, context)
}
