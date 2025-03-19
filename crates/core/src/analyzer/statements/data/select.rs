/// Analyzes a SELECT statement and determines its result type.
///
/// # Features
///
/// - Field resolution and type checking
/// - Support for aliases and wildcards
/// - FETCH clause analysis
/// - Graph traversal validation
/// - Destructuring support
///
/// # Examples
///
/// ```rust
/// # use surrealguard_core::prelude::*;
/// let mut ctx = AnalyzerContext::new();
///
/// // Basic SELECT
/// let query = "SELECT name, age FROM user;";
///
/// // With FETCH
/// let query = "SELECT post, author FROM post FETCH author;";
///
/// // Graph traversal
/// let query = "SELECT ->posted->post.* FROM user;";
/// ```
///
/// # Type Resolution
///
/// The returned type depends on the query structure:
/// - Regular SELECT: array<{fields}>
/// - SELECT VALUE: array<type>
/// - With FETCH: Expanded record types
/// - Graph queries: Nested object structure
use crate::analyzer::{
    context::AnalyzerContext,
    error::{AnalyzerError, AnalyzerResult},
};
use std::collections::BTreeMap;
use surrealdb::sql::{
    statements::{DefineStatement, SelectStatement},
    Dir, Fetch, Field, Idiom, Idioms, Kind, Literal, Part, Table, Value,
};

pub fn analyze_select(context: &AnalyzerContext, stmt: &SelectStatement) -> AnalyzerResult<Kind> {
    let table_value = stmt.what.0.first().ok_or(AnalyzerError::UnexpectedSyntax)?;
    let raw_table_name = match table_value {
        Value::Table(t) => t.0.clone(),
        Value::Thing(thing) => thing.tb.clone(),
        Value::Param(p) => match p.trim() {
            "$auth" => match context.auth() {
                Some(auth) => auth.to_string(),
                None => return Err(AnalyzerError::MissingAuth),
            },
            "$token" => {
                todo!("Implement token inference")
            }
            _ => return Err(AnalyzerError::UnexpectedSyntax),
        },
        _ => return Err(AnalyzerError::UnexpectedSyntax),
    };
    let table_name = if raw_table_name.contains(':') {
        raw_table_name.split(':').next().unwrap().to_string()
    } else {
        raw_table_name
    };

    if context.find_table_definition(&table_name).is_none() {
        return Err(AnalyzerError::TableNotFound(table_name));
    }

    let is_value_select = stmt.expr.1;

    if is_value_select {
        if stmt.expr.0.len() != 1 {
            return Err(AnalyzerError::UnexpectedSyntax);
        }
        match &stmt.expr.0[0] {
            Field::Single { expr, .. } => {
                let field_idiom = match expr {
                    Value::Idiom(idiom) => idiom,
                    _ => return Err(AnalyzerError::UnexpectedSyntax),
                };

                if let Some(DefineStatement::Field(field_def)) =
                    context.find_field_definition(&table_name, field_idiom)
                {
                    let mut resolved = field_def.kind.clone().unwrap_or(Kind::Any);
                    if let Some(fetches) = stmt.fetch.as_ref() {
                        let fetch_chain = fetches_to_chain(fetches);
                        resolved = resolved.resolve_fetch(&fetch_chain, context);
                    }
                    return Ok(Kind::Literal(Literal::Array(vec![resolved])));
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

    let base_kind = if stmt.expr.0.is_empty() || stmt.expr.0.iter().any(|f| matches!(f, Field::All))
    {
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

                    // Check if this is a graph traversal by looking for Graph parts
                    if field_idiom.0.iter().any(|p| matches!(p, Part::Graph(_))) {
                        let graph_type = analyze_graph_path(context, field_idiom)?;

                        if let Some(alias_name) = alias {
                            // For aliased paths, extract the innermost array type
                            if let Kind::Literal(Literal::Object(graph_fields)) = graph_type {
                                let final_type = extract_final_type(&graph_fields);
                                field_types.insert(alias_name.to_string(), final_type);
                            }
                        } else {
                            // No alias - use the full path structure
                            if let Kind::Literal(Literal::Object(graph_fields)) = graph_type {
                                field_types.extend(graph_fields);
                            }
                        }
                        continue;
                    }

                    // Handle destructuring
                    if let Some((parent_path, fields)) = get_destructure_parts(field_idiom) {
                        if let Some(DefineStatement::Field(parent_field_def)) =
                            context.find_field_definition(&table_name, &parent_path)
                        {
                            if let Some(Kind::Literal(Literal::Object(parent_type))) =
                                &parent_field_def.kind
                            {
                                let mut destructured_types = BTreeMap::new();
                                for field_name in fields {
                                    if let Some(field_type) = parent_type.get(&field_name) {
                                        destructured_types.insert(field_name, field_type.clone());
                                    }
                                }
                                let output_name = if let Some(alias_name) = alias {
                                    alias_name.to_string()
                                } else {
                                    parent_path.to_string()
                                };
                                field_types.insert(
                                    output_name,
                                    Kind::Literal(Literal::Object(destructured_types)),
                                );
                                continue;
                            }
                        }
                    }

                    // Regular field handling
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

    let transformed_kind = if let Some(fetches) = stmt.fetch.as_ref() {
        let fetch_chain = fetches_to_chain(fetches);
        base_kind.resolve_fetch(&fetch_chain, context)
    } else {
        base_kind
    };

    if stmt.only {
        // For SELECT ONLY, return the record type as is.
        Ok(transformed_kind)
    } else {
        // For a normal SELECT, wrap the record type in an Array.
        Ok(Kind::Array(Box::new(transformed_kind), None))
    }
}

fn extract_final_type(fields: &BTreeMap<String, Kind>) -> Kind {
    // We expect only one key in each level
    if let Some((_key, value)) = fields.iter().next() {
        match value {
            Kind::Literal(Literal::Object(inner_fields)) => {
                // Recurse into nested objects
                extract_final_type(inner_fields)
            }
            Kind::Literal(Literal::Array(array_types)) => {
                // We found the final array - return it
                Kind::Literal(Literal::Array(array_types.clone()))
            }
            // For any other type, return as is
            other => other.clone(),
        }
    } else {
        // Shouldn't happen with valid graph types
        Kind::Any
    }
}

/// Specifies an optional modifier on the final graph segment.
enum Modifier {
    All,
    Destructure(Vec<String>),
}

/// Analyzes a graph traversal path (for example,
/// `"SELECT ->memberOf->org FROM user;"` or
/// `"SELECT <-memberOf<-user.* FROM org;"`)
/// and produces the corresponding nested type.
///
/// The algorithm first looks for a `Part::Graph` in the idiom. If found, it uses its string
/// representation and calls `parse_graph` to break it into segments (each with a direction and table)
/// and an optional modifier (either “all” or a destructuring list) for the last segment. Then the
/// innermost type is built from the last segment (using the full table type if a modifier is present,
/// or a record link if not), and finally the remaining segments are wrapped outward as nested objects.
///
/// # Errors
/// Returns an error if no graph parts are present.
pub fn analyze_graph_path(context: &AnalyzerContext, field_idiom: &Idiom) -> AnalyzerResult<Kind> {
    // Find all Graph parts and the final modifier
    let mut final_modifier = None;
    let graph_parts: Vec<_> = field_idiom
        .0
        .iter()
        .filter_map(|part| match part {
            Part::Graph(g) => Some(g),
            Part::All => {
                final_modifier = Some(Modifier::All);
                None
            }
            Part::Destructure(fields) => {
                final_modifier = Some(Modifier::Destructure(
                    fields.iter().map(|p| p.to_string()).collect(),
                ));
                None
            }
            _ => None,
        })
        .collect();

    if graph_parts.is_empty() {
        return Err(AnalyzerError::UnexpectedSyntax);
    }

    // For single graph part
    if graph_parts.len() == 1 {
        let graph = &graph_parts[0];
        let table = &graph.what.0[0].0;

        // Handle modifiers for the record type
        let inner_type = match final_modifier {
            Some(Modifier::All) => build_full_table_type(context, table, None)?,
            Some(Modifier::Destructure(ref fields)) => {
                let full = build_full_table_type(context, table, None)?;
                restrict_type(full, fields)
            }
            None => Kind::Record(vec![Table::from(table.clone())]),
        };

        let mut result = BTreeMap::new();
        let key = match graph.dir {
            Dir::In => format!("<-{}", table),
            Dir::Out => format!("->{}", table),
            _ => return Err(AnalyzerError::UnexpectedSyntax),
        };

        result.insert(key, Kind::Literal(Literal::Array(vec![inner_type])));
        return Ok(Kind::Literal(Literal::Object(result)));
    }

    // For multiple graph parts, build from inside out
    let mut current = {
        let last_graph = graph_parts.last().unwrap();
        let table = &last_graph.what.0[0].0;

        // Handle modifiers for the final record type
        let inner_type = match final_modifier {
            Some(Modifier::All) => build_full_table_type(context, table, None)?,
            Some(Modifier::Destructure(ref fields)) => {
                let full = build_full_table_type(context, table, None)?;
                restrict_type(full, fields)
            }
            None => Kind::Record(vec![Table::from(table.clone())]),
        };

        let mut map = BTreeMap::new();
        let key = match last_graph.dir {
            Dir::In => format!("<-{}", table),
            Dir::Out => format!("->{}", table),
            _ => return Err(AnalyzerError::UnexpectedSyntax),
        };

        map.insert(key, Kind::Literal(Literal::Array(vec![inner_type])));
        Kind::Literal(Literal::Object(map))
    };

    // Work backwards through the remaining parts
    for graph in graph_parts[..graph_parts.len() - 1].iter().rev() {
        let table = &graph.what.0[0].0;

        let mut map = BTreeMap::new();
        let key = match graph.dir {
            Dir::In => format!("<-{}", table),
            Dir::Out => format!("->{}", table),
            _ => return Err(AnalyzerError::UnexpectedSyntax),
        };

        map.insert(key, current);
        current = Kind::Literal(Literal::Object(map));
    }

    Ok(current)
}

/// Restricts a full table type (assumed to be a Literal::Object) to only include the given list of fields.
/// If the type is not a literal object, it is returned unchanged.
fn restrict_type(kind: Kind, fields: &Vec<String>) -> Kind {
    match kind {
        Kind::Literal(Literal::Object(map)) => {
            let new_map = map
                .into_iter()
                .filter(|(k, _)| fields.contains(k))
                .collect();
            Kind::Literal(Literal::Object(new_map))
        }
        other => other,
    }
}

fn get_destructure_parts(idiom: &Idiom) -> Option<(Idiom, Vec<String>)> {
    let parts = &idiom.0;
    for (i, part) in parts.iter().enumerate() {
        if let Part::Destructure(fields) = part {
            let parent_parts = parts[..i].to_vec();
            let parent_path = Idiom::from(parent_parts);

            // Since we can't match on DestructurePart variants,
            // we'll just convert the fields to strings directly
            let field_names = fields.iter().map(|p| p.to_string()).collect();

            return Some((parent_path, field_names));
        }
    }
    None
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

pub trait KindFetchExt {
    /// Resolve a fetch chain. In a FETCH context, record links (or arrays thereof)
    /// are replaced with the full table “schema” (as a Literal object).
    fn resolve_fetch(&self, fetch_chain: &[String], ctx: &AnalyzerContext) -> Self;
}

// Revised fetch resolution logic.
impl KindFetchExt for Kind {
    fn resolve_fetch(&self, fetch_chain: &[String], ctx: &AnalyzerContext) -> Self {
        // When no fetch segments remain, if self is a record we try to expand it.
        if fetch_chain.is_empty() {
            if let Kind::Record(tables) = self {
                if let Some(table) = tables.first() {
                    if let Ok(full_type) = ctx.build_full_table_type(&table.0) {
                        return full_type;
                    }
                }
            }
            return self.clone();
        }
        match self {
            Kind::Record(tables) => {
                let target = fetch_chain[0].trim().to_lowercase();
                if let Some(table) = tables.iter().find(|t| t.0.trim().to_lowercase() == target) {
                    if let Ok(full_type) = ctx.build_full_table_type(&table.0) {
                        return full_type.resolve_fetch(&fetch_chain[1..], ctx);
                    }
                }
                self.clone()
            }
            Kind::Literal(Literal::Object(map)) => {
                let mut new_map = map.clone();
                for (key, value) in map.iter() {
                    if key.trim().to_lowercase() == fetch_chain[0] {
                        // If value is an array of records, handle it specially
                        if let Kind::Array(inner, _) = value {
                            if let Kind::Record(tables) = &**inner {
                                if let Some(table) = tables.first() {
                                    if let Ok(full_type) = ctx.build_full_table_type(&table.0) {
                                        new_map.insert(
                                            key.clone(),
                                            Kind::Literal(Literal::Array(vec![full_type])),
                                        );
                                        continue;
                                    }
                                }
                            }
                        }
                        new_map.insert(key.clone(), value.resolve_fetch(&fetch_chain[1..], ctx));
                    }
                }
                Kind::Literal(Literal::Object(new_map))
            }
            Kind::Array(inner, len) => {
                let new_inner = inner.resolve_fetch(fetch_chain, ctx);
                Kind::Array(Box::new(new_inner), *len)
            }
            Kind::Either(kinds) => {
                let new_kinds = kinds
                    .iter()
                    .map(|k| k.resolve_fetch(fetch_chain, ctx))
                    .collect();
                Kind::Either(new_kinds)
            }
            _ => self.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        analyzer::{analyze, context::AnalyzerContext},
        prelude::AnalyzerResult,
    };
    use surrealdb::sql::{Kind, Statement};
    use surrealguard_macros::kind;

    // Wrapper over analyze_select that unwraps other statement types.
    fn analyze_select(ctx: &mut AnalyzerContext, query: &str) -> AnalyzerResult<Kind> {
        let stmt = surrealdb::sql::parse(query)
            .expect("Statement should be valid SurrealQL")
            .into_iter()
            .next()
            .expect("Expected at least one statement");
        let Statement::Select(stmt) = stmt else {
            panic!("Expected a SELECT statement");
        };

        super::analyze_select(ctx, &stmt)
    }

    #[test]
    fn basic() {
        let stmt = "SELECT name, age FROM user;";

        let mut ctx = AnalyzerContext::new();
        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
                DEFINE FIELD age ON user TYPE number;
        "#,
        )
        .expect("Schema construction should succeed");

        let analyzed_kind = analyze_select(&mut ctx, stmt).expect("Analysis should succeed");
        let expected_kind = kind!("array<{ name: string, age: number }>");

        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn full() {
        let stmt = "SELECT * FROM user;";

        let mut ctx = AnalyzerContext::new();
        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
                DEFINE FIELD age ON user TYPE number;
                DEFINE FIELD address ON user TYPE {
                    city: string,
                    state: string,
                    zip: number,
                    country: string
                };
        "#,
        )
        .expect("Schema construction should succeed");

        let analyzed_kind = analyze_select(&mut ctx, stmt).expect("Analysis should succeed");
        let expected_kind = kind!(
            r#"array<{
            name: string,
            age: number,
            address: {
                city: string,
                state: string,
                zip: number,
                country: string
            }
        }>"#
        );

        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn alias() {
        let stmt = "SELECT name as nom FROM user;";

        let mut ctx = AnalyzerContext::new();
        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
                DEFINE FIELD age ON user TYPE number;
        "#,
        )
        .expect("Schema construction should succeed");

        let analyzed_kind = analyze_select(&mut ctx, stmt).expect("Analysis should succeed");
        let expected_kind = kind!("array<{ nom: string }>");

        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn omit() {
        let stmt = "SELECT * OMIT age, address.zip FROM user;";

        let mut ctx = AnalyzerContext::new();
        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
                DEFINE FIELD age ON user TYPE number;
                DEFINE FIELD address ON user TYPE {
                    city: string,
                    state: string,
                    zip: number,
                    country: string
                };
        "#,
        )
        .expect("Schema construction should succeed");

        let analyzed_kind = analyze_select(&mut ctx, stmt).expect("Analysis should succeed");
        let expected_kind = kind!(
            r#"array<{
            name: string,
            address: {
                city: string,
                state: string,
                country: string
            }
        }>"#
        );

        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn fetch_record_link() {
        let schema = r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
                DEFINE FIELD age ON user TYPE number;
            DEFINE TABLE post SCHEMAFULL;
                DEFINE FIELD author ON post TYPE record<user>;
        "#;
        let mut ctx = AnalyzerContext::new();
        analyze(&mut ctx, schema).expect("Schema construction should succeed");

        let query = "SELECT author FROM post FETCH author;";
        let analyzed_kind = analyze_select(&mut ctx, query).expect("Analysis should succeed");
        let expected_kind = kind!("array<{ author: { name: string, age: number } }>");

        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn fetch_array_of_record_links() {
        let schema = r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD username ON user TYPE string;
                DEFINE FIELD email ON user TYPE string;
            DEFINE TABLE group SCHEMAFULL;
                DEFINE FIELD members ON group TYPE array<record<user>>;
        "#;
        let mut ctx = AnalyzerContext::new();
        analyze(&mut ctx, schema).expect("Schema construction should succeed");

        let query = "SELECT members FROM group FETCH members;";
        let analyzed_kind = analyze_select(&mut ctx, query).expect("Analysis should succeed");
        let expected_kind = kind!("array<{ members: [ { username: string, email: string } ] }>");

        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn select_value() {
        let query = "SELECT VALUE email FROM user;";
        let schema = r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD email ON user TYPE string;
        "#;
        let mut ctx = AnalyzerContext::new();
        analyze(&mut ctx, schema).expect("Schema construction should succeed");
        let analyzed_kind = analyze_select(&mut ctx, query).expect("Analysis should succeed");
        // Changed to match the actual structure: Array(Literal(Array([String])), None)
        let expected_kind = kind!("[string]");
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn select_only() {
        let schema = r#"
            DEFINE TABLE person SCHEMAFULL;
                DEFINE FIELD name ON person TYPE string;
                DEFINE FIELD age ON person TYPE number;
        "#;
        let mut ctx = AnalyzerContext::new();
        analyze(&mut ctx, schema).expect("Schema construction should succeed");

        let query = "SELECT * FROM ONLY person:tobie;";
        let analyzed_kind = analyze_select(&mut ctx, query).expect("Analysis should succeed");
        let expected_kind = kind!(
            r#"{
            name: string,
            age: number
        }"#
        );
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn destructuring() {
        let stmt = "SELECT address.{city, country} FROM user;";

        let mut ctx = AnalyzerContext::new();
        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD address ON user TYPE {
                    city: string,
                    state: string,
                    zip: number,
                    country: string
                };
        "#,
        )
        .expect("Schema construction should succeed");

        let analyzed_kind = analyze_select(&mut ctx, stmt).expect("Analysis should succeed");
        let expected_kind = kind!(
            r#"array<{
            address: {
                city: string,
                country: string
            }
        }>"#
        );

        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn destructuring_with_alias() {
        let stmt = "SELECT address.{city, country} AS location FROM user;";

        let mut ctx = AnalyzerContext::new();
        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD address ON user TYPE {
                    city: string,
                    state: string,
                    zip: number,
                    country: string
                };
        "#,
        )
        .expect("Schema construction should succeed");

        let analyzed_kind = analyze_select(&mut ctx, stmt).expect("Analysis should succeed");
        let expected_kind = kind!(
            r#"array<{
            location: {
                city: string,
                country: string
            }
        }>"#
        );

        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn graph_traversal_simple() {
        let mut ctx = AnalyzerContext::new();

        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE org SCHEMAFULL;
                DEFINE FIELD name ON org TYPE string;
            DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
        "#,
        )
        .expect("Schema construction should succeed");

        let stmt = "SELECT ->memberOf FROM user;";
        let analyzed_kind = analyze_select(&mut ctx, stmt).expect("Analysis should succeed");
        let expected_kind = kind!(
            r#"array<{
            "->memberOf": [record<memberOf>]
        }>"#
        );
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn graph_traversal_multi_hop() {
        let mut ctx = AnalyzerContext::new();

        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
            DEFINE TABLE team SCHEMAFULL;
                DEFINE FIELD name ON team TYPE string;
            DEFINE TABLE org SCHEMAFULL;
                DEFINE FIELD name ON org TYPE string;
                DEFINE FIELD industry ON org TYPE string;
            DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO team;
            DEFINE TABLE partOf SCHEMAFULL TYPE RELATION FROM team TO org;
        "#,
        )
        .expect("Schema construction should succeed");

        let stmt = "SELECT ->memberOf->partOf->org.* FROM user;";
        let analyzed_kind = analyze_select(&mut ctx, stmt).expect("Analysis should succeed");
        let expected_kind = kind!(
            r#"array<{
            "->memberOf": {
                "->partOf": {
                    "->org": [{
                        name: string,
                        industry: string
                    }]
                }
            }
        }>"#
        );

        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn graph_traversal_to_node() {
        let mut ctx = AnalyzerContext::new();

        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE org SCHEMAFULL;
                DEFINE FIELD name ON org TYPE string;
            DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
        "#,
        )
        .expect("Schema construction should succeed");

        let stmt = "SELECT ->memberOf->org FROM user;";
        let analyzed_kind = analyze_select(&mut ctx, stmt).expect("Analysis should succeed");
        let expected_kind = kind!(
            r#"array<{
            "->memberOf": {
                "->org": [record<org>]
            }
        }>"#
        );
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn graph_traversal_with_fields() {
        let mut ctx = AnalyzerContext::new();

        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE org SCHEMAFULL;
                DEFINE FIELD name ON org TYPE string;
            DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
        "#,
        )
        .expect("Schema construction should succeed");

        let stmt = "SELECT ->memberOf->org.* FROM user;";
        let analyzed_kind = analyze_select(&mut ctx, stmt).expect("Analysis should succeed");
        let expected_kind = kind!(
            r#"array<{
            "->memberOf": {
                "->org": [{
                    name: string
                }]
            }
        }>"#
        );
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn graph_traversal_with_destructure() {
        let mut ctx = AnalyzerContext::new();

        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
            DEFINE TABLE org SCHEMAFULL;
                DEFINE FIELD name ON org TYPE string;
                DEFINE FIELD address ON org TYPE string;
            DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
        "#,
        )
        .expect("Schema construction should succeed");

        let stmt = "SELECT ->memberOf->org.{name} FROM user;";
        let analyzed_kind = analyze_select(&mut ctx, stmt).expect("Analysis should succeed");
        let expected_kind = kind!(
            r#"array<{
            "->memberOf": {
                "->org": [{
                    name: string
                }]
            }
        }>"#
        );
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn graph_traversal_reverse() {
        let mut ctx = AnalyzerContext::new();

        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
            DEFINE TABLE org SCHEMAFULL;
            DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
        "#,
        )
        .expect("Schema construction should succeed");

        let stmt = "SELECT <-memberOf<-user.* FROM org;";
        let analyzed_kind = analyze_select(&mut ctx, stmt).expect("Analysis should succeed");
        let expected_kind = kind!(
            r#"array<{
            "<-memberOf": {
                "<-user": [{
                    name: string
                }]
            }
        }>"#
        );
        assert_eq!(analyzed_kind, expected_kind);
    }

    #[test]
    fn graph_traversal_with_alias() {
        let mut ctx = AnalyzerContext::new();

        analyze(
            &mut ctx,
            r#"
            DEFINE TABLE user SCHEMAFULL;
                DEFINE FIELD name ON user TYPE string;
            DEFINE TABLE org SCHEMAFULL;
                DEFINE FIELD name ON org TYPE string;
                DEFINE FIELD industry ON org TYPE string;
            DEFINE TABLE memberOf SCHEMAFULL TYPE RELATION FROM user TO org;
        "#,
        )
        .expect("Schema construction should succeed");

        let stmt = "SELECT ->memberOf->org.* AS orgs FROM user;";
        let analyzed_kind = analyze_select(&mut ctx, stmt).expect("Analysis should succeed");
        let expected_kind = kind!(
            r#"array<{
            "orgs": [{
                name: string,
                industry: string
            }]
        }>"#
        );

        assert_eq!(analyzed_kind, expected_kind);
    }
}
