use crate::config::Config;
use crate::error::{CodegenError, Result};
use std::collections::{HashMap, BTreeMap};
use std::fs;
use std::path::{Path, PathBuf};
use regex::Regex;
use walkdir::WalkDir;
use surrealguard_core::analyzer::{self, context::AnalyzerContext};
use surrealdb::sql::{Kind, Literal};

pub struct Generator {
    ctx: AnalyzerContext,
    query_types: HashMap<String, QueryInfo>,
}

#[derive(Debug, Clone)]
/// QueryInfo holds the metadata for each discovered query. For queries discovered from files,
/// the "name" field is set (converted to PascalCase) so that we can export a constant for it.
struct QueryInfo {
    /// If available (for file-based queries) this is the PascalCase query name.
    pub name: Option<String>,
    /// The SQL query text (this is used as the key in the generated Queries mapping).
    pub query: String,
    /// The generated type definition for the query’s result, always inlined into the mapping.
    pub type_def: String,
    /// A string representing the variables type (if any), or `None` if no variables are inferred.
    pub variables_type: Option<String>,
    /// A doc comment showing the analyzed kind.
    pub doc_comment: String,
}

impl Generator {
    pub fn new() -> Self {
        Self {
            ctx: AnalyzerContext::new(),
            query_types: HashMap::new(),

        }
    }

    pub fn check(&mut self, config: &Config) -> Result<()> {
        self.load_schema(&config.schema.path)?;

        if let Some(queries_path) = &config.queries.path {
            self.process_queries(queries_path)?;
        }

        if let Some(src_dirs) = &config.queries.src {
            self.scan_source_files(src_dirs)?;
        }

        Ok(())
    }

    pub fn generate(&mut self, config: &Config) -> Result<()> {
        self.load_schema(&config.schema.path)?;

        if let Some(queries_path) = &config.queries.path {
            self.process_queries(queries_path)?;
        }

        if let Some(src_dirs) = &config.queries.src {
            self.scan_source_files(src_dirs)?;
        }

        self.generate_output(&config.output.path, config.output.format)
    }

    fn load_schema(&mut self, path: &Path) -> Result<()> {
        if path.is_dir() {
            for entry in WalkDir::new(path) {
                let entry = entry.map_err(|_| CodegenError::InvalidPath(path.to_path_buf()))?;
                if entry.path().extension().map_or(false, |ext| ext == "surql") {
                    let content = fs::read_to_string(entry.path())?;
                    analyzer::analyze(&mut self.ctx, &content)
                        .map_err(|e| CodegenError::Analysis {
                            error: Box::new(e),
                            context: format!("schema file: {}", entry.path().display()),
                        })?;
                }
            }
        } else {
            let content = fs::read_to_string(path)?;
            analyzer::analyze(&mut self.ctx, &content)
                .map_err(|e| CodegenError::Analysis {
                    error: Box::new(e),
                    context: format!("schema file: {}", path.display()),
                })?;
        }
        Ok(())
    }

    fn process_queries(&mut self, path: &Path) -> Result<()> {
        if path.is_dir() {
            for entry in WalkDir::new(path) {
                let entry = entry.map_err(|_| CodegenError::InvalidPath(path.to_path_buf()))?;
                if entry.path().extension().map_or(false, |ext| ext == "surql") {
                    self.process_query_file(entry.path())?;
                }
            }
        } else {
            self.process_query_file(path)?;
        }
        Ok(())
    }

    fn process_query_file(&mut self, path: &Path) -> Result<()> {
        let content = fs::read_to_string(path)?;
        // File-based queries use the file stem as their name.
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("UnnamedQuery")
            .to_string();
        self.analyze_query(&content, Some(name))
            .map_err(|e| match e {
                CodegenError::Analysis { error, .. } => CodegenError::Analysis {
                    error,
                    context: format!("query file: {}", path.display()),
                },
                other => other,
            })
    }

    // Fuck typescript but we have to do this since template literals are borked:
    // https://github.com/microsoft/TypeScript/issues/33304
    fn scan_source_files(&mut self, dirs: &[PathBuf]) -> Result<()> {
        // Match surql( ... ) where the query is provided as a literal string.
        // The regex will match either:
        //   surql("query")
        //   surql('query')
        //   surql(`query`)
        //
        // Capture group 1 matches double quotes,
        // group 2 matches single quotes,
        // group 3 matches backticks.
        let re = Regex::new(r#"surql\(\s*(?:"([^"]*)"|'([^']*)'|`([^`]*)`)"#)
            .expect("invalid regex");

        for dir in dirs {
            for entry in WalkDir::new(dir) {
                let entry = entry.map_err(|_| CodegenError::InvalidPath(dir.clone()))?;
                if let Some(ext) = entry.path().extension() {
                    match ext.to_str() {
                        Some("ts" | "js" | "jsx" | "tsx" | "svelte" | "vue") => {
                            let content = fs::read_to_string(entry.path())?;
                            for cap in re.captures_iter(&content) {
                                // Try the three capture groups in order.
                                let query_candidate = cap.get(1)
                                    .or_else(|| cap.get(2))
                                    .or_else(|| cap.get(3));
                                if let Some(m) = query_candidate {
                                    let query = m.as_str().trim();
                                    if query.is_empty() {
                                        continue;
                                    }
                                    // Analyze the query string.
                                    self.analyze_query(query, None)
                                        .map_err(|e| match e {
                                            CodegenError::Analysis { error, .. } => CodegenError::Analysis {
                                                error,
                                                context: format!("source file: {} (query: {})", entry.path().display(), query),
                                            },
                                            other => other,
                                        })?;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    // analyze_query analyzes the SQL query and generates its type definition.
    // If a name is provided (such as for file-based queries), it is converted to PascalCase.
    fn analyze_query(&mut self, query: &str, name: Option<String>) -> Result<()> {
        let mut ctx = self.ctx.clone();
        let kind = analyzer::analyze(&mut ctx, query)
            .map_err(|e| CodegenError::Analysis {
                error: Box::new(e),
                context: format!("analyzing query: {}", query),
            })?;
        let type_def = self.generate_type(&kind);

        let query_name = name.map(|n| {
            n.split('_')
             .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    None => String::new(),
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                }
             })
             .collect::<String>()
        });

        // Get inferred parameter types (if any).
        let variables_type = if !ctx.get_all_inferred_params().is_empty() {
            let params = ctx.get_all_inferred_params();
            let fields: Vec<String> = params
                .iter()
                .map(|(name, kind)| format!("    {}: {}", name, self.generate_type(kind)))
                .collect();
            Some(format!("{{\n{}\n}}", fields.join(",\n")))
        } else {
            None
        };

        let doc_comment = format!(
            "/**\n * ## Query results\n *\n * Kind:\n * ```\n * {}\n * ```\n */",
            kind.to_string()
        );

        let info = QueryInfo {
            name: query_name,
            query: query.to_string(),
            type_def,
            variables_type,
            doc_comment,
        };

        // Use the query text itself as the key in the generated Queries mapping.
        self.query_types.insert(info.query.clone(), info);
        Ok(())
    }

    // generate_type converts a Kind to its corresponding TypeScript type definition.
    fn generate_type(&self, kind: &Kind) -> String {
        match kind {
            Kind::Null => "null".to_string(),
            Kind::Bool => "boolean".to_string(),
            Kind::Number => "number".to_string(),
            Kind::String => "string".to_string(),
            Kind::Datetime => "Date".to_string(),
            Kind::Duration => "Duration".to_string(),
            Kind::Uuid => "string".to_string(),
            Kind::Bytes => "Uint8Array".to_string(),
            Kind::Array(inner, _) => format!("Array<{}>", self.generate_type(inner)),
            Kind::Object => "Record<string, any>".to_string(),
            Kind::Record(tables) => {
                if let Some(table) = tables.first() {
                    format!("RecordId<\"{}\">", table.0)
                } else {
                    "RecordId<string>".to_string()
                }
            }
            Kind::Literal(lit) => self.generate_literal(lit),
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
            Kind::Either(kinds) => {
                let types: Vec<String> = kinds.iter().map(|k| self.generate_type(k)).collect();
                types.join(" | ")
            }
            Kind::Option(inner) => {
                let inner_type = self.generate_type(inner);
                format!("({} | undefined)", inner_type)
            }
            _ => "any".to_string(),
        }
    }

    // generate_literal converts a Literal to its TypeScript literal representation.
    fn generate_literal(&self, lit: &Literal) -> String {
        match lit {
            Literal::String(s) => format!("\"{}\"", s),
            Literal::Number(n) => n.to_string(),
            Literal::Duration(d) => format!("Duration.from(\"{}\")", d),
            Literal::Array(items) => {
                let types: Vec<String> = items.iter().map(|item| self.generate_type(item)).collect();
                format!("[{}]", types.join(", "))
            }
            Literal::Object(fields) => {
                // Nest dotted field names (e.g., "email.address" -> email: { address: ... })
                let nested_fields = self.nest_dotted_fields(fields);
                let field_defs: Vec<String> = nested_fields
                    .iter()
                    .map(|(name, kind)| {
                        let value = self.generate_type(kind);
                        if value.contains('\n') {
                            format!("  {}: {}", name, value.replace('\n', "\n  "))
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
            Literal::DiscriminatedObject(discriminant, variants) => {
                // Handle union types like { type: "system", ... } | { type: "user", ... }
                let variant_types: Vec<String> = variants
                    .iter()
                    .map(|variant| {
                        let nested_fields = self.nest_dotted_fields(variant);
                        let field_defs: Vec<String> = nested_fields
                            .iter()
                            .map(|(name, kind)| {
                                let value = self.generate_type(kind);
                                format!("    {}: {}", name, value)
                            })
                            .collect();
                        if field_defs.is_empty() {
                            "{}".to_string()
                        } else {
                            format!("{{\n{}\n  }}", field_defs.join(",\n"))
                        }
                    })
                    .collect();
                variant_types.join(" | ")
            }
            _ => "any".to_string(),
        }
    }

    // nest_dotted_fields converts dotted field names like "email.address" into nested objects
    fn nest_dotted_fields(&self, fields: &BTreeMap<String, Kind>) -> BTreeMap<String, Kind> {
        
        let mut nested = BTreeMap::new();
        let mut dotted_groups: BTreeMap<String, BTreeMap<String, Kind>> = BTreeMap::new();
        
        for (name, kind) in fields {
            if name.contains('.') {
                let parts: Vec<&str> = name.splitn(2, '.').collect();
                if parts.len() == 2 {
                    let parent = parts[0].to_string();
                    let child = parts[1].to_string();
                    
                    dotted_groups
                        .entry(parent)
                        .or_insert_with(BTreeMap::new)
                        .insert(child, kind.clone());
                } else {
                    nested.insert(name.clone(), kind.clone());
                }
            } else {
                // Check if this field has dotted children
                let has_dotted_children = fields.keys().any(|k| k.starts_with(&format!("{}.", name)));
                if !has_dotted_children {
                    nested.insert(name.clone(), kind.clone());
                }
            }
        }
        
        // Convert dotted groups to nested objects
        for (parent, children) in dotted_groups {
            let nested_children = self.nest_dotted_fields(&children);
            nested.insert(parent, Kind::Literal(Literal::Object(nested_children)));
        }
        
        nested
    }

    // escape_string_literal escapes backticks and other important characters.
    fn escape_string_literal(s: &str) -> String {
        s.replace('\\', "\\\\")
         .replace('\"', "\\\"")
         .replace('\n', "\\n")
         .replace('\r', "\\r")
         .replace('\t', "\\t")
    }

    // generate_output writes out the unified TypeScript definitions.
    // In the Queries mapping, every query's result type is inlined.
    // For queries discovered from files (with a name) we also export constants.
    fn generate_output(&self, path: &Path, should_format: bool) -> Result<()> {
        let mut content = String::new();
        content.push_str("import { type RecordId, Surreal } from 'surrealdb';\n\n");

        // Generate a unified Queries type mapping SQL strings to their definitions.
        content.push_str("export type Queries = {\n");
        for info in self.query_types.values() {
            content.push_str(&format!(
                "    \"{}\": {{ variables: {}, result: {} }};\n",
                Self::escape_string_literal(&info.query),
                info.variables_type.as_deref().unwrap_or("never"),
                // Always inline the type definition here.
                info.type_def
            ));
        }
        content.push_str("};\n\n");

        // Helper type for enforcing variables via rest parameters.
        content.push_str("export type Variables<Q extends keyof Queries> = Queries[Q]['variables'] extends never ? [] : [Queries[Q]['variables']];\n\n");

        // For queries that came from a file (with a provided name), export the queries as named constants.
        for info in self.query_types.values() {
            if let Some(name) = &info.name {
                content.push_str(&info.doc_comment);
                content.push('\n');

                if let Some(vars) = &info.variables_type {
                    content.push_str(&format!(
                        "export interface {}Variables {}\n\n",
                        name, vars
                    ));
                }

                content.push_str(&format!(
                    "export const {} = `{}`;\n\n",
                    name,
                    info.query.replace('`', "\\`")
                ));
            }
        }

        // Generate the utility class and the tagged template helper.
        content.push_str(
r#"export class TypedSurreal extends Surreal {
    typed<Q extends keyof Queries>(query: Q, ...rest: Variables<Q>): Promise<Queries[Q]["result"]> {
        return this.query(query, rest[0]);
    }

    async query(sql: string, vars?: any): Promise<any> {
        return super.query(sql, vars);
    }
}

export function surql<Q extends keyof Queries>(query: Q): Q {
  return query;
}
"#
        );

        fs::write(path, content)?;

        if should_format {
            let output = std::process::Command::new("prettier")
                .arg("--write")
                .arg(path)
                .output()
                .map_err(|e| CodegenError::Format(e.to_string()))?;

            if !output.status.success() {
                let error = String::from_utf8_lossy(&output.stderr);
                return Err(CodegenError::Format(error.to_string()));
            }
        }

        Ok(())
    }
}
