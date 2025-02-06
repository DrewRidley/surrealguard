use surrealdb::sql::{Kind, Literal};

pub struct TypeScriptGenerator;

impl TypeScriptGenerator {
    pub fn generate(kind: &Kind) -> String {
        match kind {
            // Basic types
            Kind::Null => "null".to_string(),
            Kind::Bool => "boolean".to_string(),
            Kind::Number => "number".to_string(),
            Kind::String => "string".to_string(),
            Kind::Datetime => "Date".to_string(),
            Kind::Duration => "Duration".to_string(), // SurrealDB Duration type
            Kind::Uuid => "string".to_string(),
            Kind::Bytes => "Uint8Array".to_string(),

            // Arrays
            Kind::Array(inner, _) => format!("Array<{}>", Self::generate(inner)),

            // Objects and Records
            Kind::Object => "Record<string, any>".to_string(),
            Kind::Record(tables) => {
                if let Some(table) = tables.first() {
                    format!("(RecordId<\"{}\"> & {{ id: string }})", table.0)
                } else {
                    "RecordId<string>".to_string()
                }
            }

            // Literals (explicit values)
            Kind::Literal(lit) => Self::generate_literal(lit),

            // Special types
            Kind::Range => "RecordIdRange".to_string(),
            Kind::Geometry(types) => {
                if let Some(geo_type) = types.first() {
                    match geo_type.to_lowercase().as_str() {
                        "point" => "Point".to_string(),
                        "line" => "Line".to_string(),
                        "polygon" => "Polygon".to_string(),
                        "multipoint" => "MultiPoint".to_string(),
                        "multiline" => "MultiLine".to_string(),
                        "multipolygon" => "MultiPolygon".to_string(),
                        "collection" => "GeometryCollection".to_string(),
                        _ => "Geometry".to_string(),
                    }
                } else {
                    "Geometry".to_string()
                }
            }

            // Union types
            Kind::Either(kinds) => {
                let types: Vec<String> = kinds.iter().map(|k| Self::generate(k)).collect();
                types.join(" | ")
            }

            // Function types (rarely needed in TS outputs)
            Kind::Function(_, _) => "Function".to_string(),

            // Fallback for Any and unhandled types
            _ => "any".to_string(),
        }
    }

    fn generate_literal(lit: &Literal) -> String {
        match lit {
            Literal::String(s) => format!("\"{}\"", s),
            Literal::Number(n) => n.to_string(),
            Literal::Duration(d) => format!("Duration.from(\"{}\")", d),
            Literal::Array(items) => {
                let types: Vec<String> = items.iter().map(|item| Self::generate(item)).collect();
                format!("[{}]", types.join(", "))
            }
            Literal::Object(fields) => {
                let field_defs: Vec<String> = fields
                    .iter()
                    .map(|(name, kind)| {
                        let value = Self::generate(kind);
                        if value.contains('\n') {
                            format!("  {}: {}", name, value.replace("\n", "\n  "))
                        } else {
                            format!("  {}: {}", name, value)
                        }
                    })
                    .collect();

                if field_defs.is_empty() {
                    "{}".to_string()
                } else {
                    format!("{{\n{}\n}}", field_defs.join(",\n"))
                }
            }
            Literal::DiscriminatedObject(_tag, variants) => {
                let variant_types: Vec<String> = variants
                    .iter()
                    .map(|variant| {
                        let fields: Vec<String> = variant
                            .iter()
                            .map(|(name, kind)| format!("  {}: {}", name, Self::generate(kind)))
                            .collect();
                        format!("{{\n{}\n}}", fields.join(";\n"))
                    })
                    .collect();
                variant_types.join(" | ")
            }
            _ => "any".to_string(),
        }
    }
}
