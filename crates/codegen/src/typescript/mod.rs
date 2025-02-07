use crate::config::Config;
use crate::error::{CodegenError, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use regex::Regex;
use walkdir::WalkDir;
use surrealguard_core::analyzer::{self, context::AnalyzerContext};
use surrealdb::sql::{Kind, Literal};

pub struct Generator {
    ctx: AnalyzerContext,
    query_types: HashMap<String, QueryInfo>,
    inline_counter: u32,
}

#[derive(Debug, Clone)]
struct QueryInfo {
    /// The “alias” name used for exporting the TS types (for inline queries a generated name is used)
    name: String,
    /// The SQL query text
    query: String,
    /// The generated type definition for the query’s result
    type_def: String,
    /// A string representing the variables type (if any) or `None`
    variables_type: Option<String>,
    /// A doc comment showing the analyzed kind
    doc_comment: String,
    /// Whether this query was found in a source file (inline) or in a file on disk
    inline: bool,
}

impl Generator {
    pub fn new() -> Self {
        Self {
            ctx: AnalyzerContext::new(),
            query_types: HashMap::new(),
            inline_counter: 0,
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
                    analyzer::analyze(&mut self.ctx, &content)?;
                }
            }
        } else {
            let content = fs::read_to_string(path)?;
            analyzer::analyze(&mut self.ctx, &content)?;
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
        // For file queries we use the file stem to generate a name but later we key the map by the SQL string.
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("UnnamedQuery")
            .to_string();
        self.analyze_query(&content, Some(name), false)
    }

    fn scan_source_files(&mut self, dirs: &[PathBuf]) -> Result<()> {
        let re = Regex::new(r#"surql`([^`]*)`"#).unwrap();

        for dir in dirs {
            for entry in WalkDir::new(dir) {
                let entry = entry.map_err(|_| CodegenError::InvalidPath(dir.clone()))?;
                if let Some(ext) = entry.path().extension() {
                    match ext.to_str() {
                        Some("ts" | "js" | "jsx" | "tsx" | "svelte" | "vue") => {
                            let content = fs::read_to_string(entry.path())?;
                            for cap in re.captures_iter(&content) {
                                if let Some(query) = cap.get(1) {
                                    self.analyze_query(query.as_str(), None, true)?;
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

    fn analyze_query(&mut self, query: &str, name: Option<String>, inline: bool) -> Result<()> {
        let kind = analyzer::analyze(&mut self.ctx, query)?;
        let type_def = self.generate_type(&kind);

        // Generate a PascalCase name if one is provided; otherwise generate an inline query name.
        // (Note: In the output map the key is the full SQL string, so file queries use their query text as the key.)
        let query_name = if let Some(n) = name {
            n.split('_')
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        None => String::new(),
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    }
                })
                .collect()
        } else {
            self.inline_counter += 1;
            format!("InlineQuery{}", self.inline_counter)
        };

        // Get inferred parameter types (if any)
        let variables_type = if !self.ctx.get_all_inferred_params().is_empty() {
            let params = self.ctx.get_all_inferred_params();
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
            inline,
        };

        // Use the query text itself as the key in the generated Queries map.
        self.query_types.insert(info.query.clone(), info);
        Ok(())
    }

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
                    format!("(RecordId<\"{}\"> & {{ id: string }})", table.0)
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
            _ => "any".to_string(),
        }
    }

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
                let field_defs: Vec<String> = fields
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
            _ => "any".to_string(),
        }
    }

    fn escape_string_literal(s: &str) -> String {
        s.replace('\\', "\\\\")
         .replace('\"', "\\\"")
         .replace('\n', "\\n")
         .replace('\r', "\\r")
         .replace('\t', "\\t")
    }

    fn generate_output(&self, path: &Path, should_format: bool) -> Result<()> {
        let mut content = String::new();
        content.push_str("import { type RecordId, Surreal } from 'surrealdb';\n\n");

        // Generate a unified Queries type mapping SQL strings to their definitions.
        content.push_str("export type Queries = {\n");
        for info in self.query_types.values() {
            content.push_str(&format!(
                "    \"{}\": {{ variables: {}, result: {}Result }};\n",
                Self::escape_string_literal(&info.query),
                info.variables_type.as_deref().unwrap_or("never"),
                info.name
            ));
        }
        content.push_str("};\n\n");

        // Generate helper type for enforcing variables via rest parameters
        content.push_str("export type Variables<Q extends keyof Queries> = Queries[Q]['variables'] extends never ? [] : [Queries[Q]['variables']];\n\n");

        // Generate the individual query definitions.
        for info in self.query_types.values() {
            content.push_str(&info.doc_comment);
            content.push('\n');

            if let Some(vars) = &info.variables_type {
                content.push_str(&format!(
                    "export interface {}Variables {}\n\n",
                    info.name, vars
                ));
            }

            content.push_str(&format!(
                "export const {} = `{}`;\n",
                info.name,
                info.query.replace('`', "\\`")
            ));
            content.push_str(&format!(
                "export type {}Result = [\n    {}\n];\n\n",
                info.name, info.type_def
            ));
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

export function surql<const S extends readonly string[]>(
    strings: TemplateStringsArray & { raw: S },
    ...values: unknown[]
): S[0] & keyof Queries {
    return strings[0] as S[0] & keyof Queries;
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
