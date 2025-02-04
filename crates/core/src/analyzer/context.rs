use surrealdb::sql::statements::{DefineFieldStatement, DefineTableStatement};
use surrealdb::sql::{statements::DefineStatement, Geometry, Kind, Table, Value};
use surrealdb::sql::{
    Idiom, Part, TableType
};

use super::error::AnalyzerResult;

pub struct AnalyzerContext {
    definitions: Vec<DefineStatement>,
}

impl AnalyzerContext {
    pub fn new() -> Self {
            Self {
                definitions: Vec::new()
            }
    }

    /// Gets the target table of a relation
        pub fn get_relation_target(&self, relation_table: &str, is_reverse: bool) -> Option<String> {
            if let Some(DefineStatement::Table(table_def)) = self.find_table_definition(relation_table) {
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

    /// Finds a relation definition (i.e. a table whose TableType is Relation)
        /// matching the given relation idiom.
        pub fn find_relation_definition(&self, relation_idiom: &Idiom) -> Option<&DefineTableStatement> {
            self.definitions.iter().find_map(|def| {
                if let DefineStatement::Table(table_def) = def {
                    // Compare the table’s name with the given relation idiom.
                    // Here we compare the normalized strings.
                    if table_def.name.to_string().eq_ignore_ascii_case(&relation_idiom.to_string())
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
                self.definitions.iter()
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

    pub fn find_field_definition(&self, table_name: &str, field_idiom: &Idiom) -> Option<&DefineStatement> {
            // Try exact match first
            let exact_match = self.definitions.iter().find(|def| {
                if let DefineStatement::Field(field_def) = def {
                    field_def.what.0 == table_name &&
                    &field_def.name == field_idiom
                } else {
                    false
                }
            });

            if exact_match.is_some() {
                return exact_match;
            }

            // If no exact match, try parent paths
            if field_idiom.0.len() > 1 {
                let parent_parts: Vec<Part> = field_idiom.0[..field_idiom.0.len()-1]
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
            Value::Number(number) => Kind::Number,
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
            },
            Value::Object(_) => Kind::Object,
            Value::Geometry(geometry) => match geometry {
                Geometry::Point(_) => Kind::Geometry(vec!["point".to_string()]),
                Geometry::Line(_) => Kind::Geometry(vec!["line".to_string()]),
                Geometry::Polygon(_) => Kind::Geometry(vec!["polygon".to_string()]),
                Geometry::MultiPoint(_) => Kind::Geometry(vec!["multipoint".to_string()]),
                Geometry::MultiLine(_) => Kind::Geometry(vec!["multiline".to_string()]),
                Geometry::MultiPolygon(_) => Kind::Geometry(vec!["multipolygon".to_string()]),
                Geometry::Collection(_) => Kind::Geometry(vec!["collection".to_string()]),
                _ => todo!(),
            },
            Value::Bytes(_) => Kind::Bytes,
            Value::Thing(thing) => Kind::Record(vec![Table::from(thing.tb.clone())]),
            Value::Table(table) => Kind::Record(vec![table.clone()]),
            Value::Range(_) => Kind::Range,
            Value::Function(_) => Kind::Function(None, None),
            Value::Model(_) => Kind::Object,
            Value::Mock(_) |
            Value::Param(_) |
            Value::Idiom(_) |
            Value::Regex(_) |
            Value::Cast(_) |
            Value::Block(_) |
            Value::Edges(_) |
            Value::Future(_) |
            Value::Constant(_) |
            Value::Subquery(_) |
            Value::Expression(_) |
            Value::Query(_) |
            Value::Closure(_) => Kind::Any,
            _ => todo!()
        })
    }
}
