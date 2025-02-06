use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;
use walkdir::{DirEntry, WalkDir};

/// Custom CLI error type.
#[derive(Error, Debug)]
pub enum CliError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Analysis error: {0}")]
    Analysis(#[from] surrealguard_core::analyzer::error::AnalyzerError),

    #[error("No valid schema files found in {0}")]
    NoSchemaFiles(PathBuf),

    #[error("No valid query files found in {0}")]
    NoQueryFiles(PathBuf),

    #[error("Invalid path: {0}")]
    InvalidPath(PathBuf),

    #[error("Walk error: {0}")]
    Walk(#[from] walkdir::Error),

    #[error("Template error: {0}")]
    Template(String),
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Path to the schema file or directory.
    #[arg(long, value_name = "PATH")]
    schema: PathBuf,

    /// Path to the query file or directory.
    #[arg(long, value_name = "PATH")]
    queries: Option<PathBuf>,

    /// One or more source directories to scan for inline queries.
    #[arg(long, value_name = "DIR")]
    src: Option<Vec<PathBuf>>,

    /// Output TypeScript file path.
    #[arg(long, default_value = "src/queries.ts")]
    output: PathBuf,

    /// Enable watch mode (not yet implemented).
    #[arg(long)]
    watch: bool,

    #[arg(long)]
    format: bool,
}

#[derive(Debug, Clone)]
struct QueryInfo {
    name: String,
    query: String,
    type_def: String,
    variables_type: Option<String>,
    /// A formatted comment including the Kind representation.
    doc_comment: String,
}

struct Generator {
    ctx: surrealguard_core::analyzer::context::AnalyzerContext,
    query_types: HashMap<String, QueryInfo>,
}

impl Generator {
    fn new() -> Self {
        println!("Initializing Generator...");
        Self {
            ctx: surrealguard_core::analyzer::context::AnalyzerContext::new(),
            query_types: HashMap::new(),
        }
    }

    fn process_entry(&mut self, entry: &DirEntry) -> Result<bool, CliError> {
        let path = entry.path();
        if path.extension().map_or(false, |ext| ext == "surql") {
            println!("Found schema file: {:?}", path);
            self.load_schema_file(path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn process_source_entry(&mut self, entry: &DirEntry, re: &Regex) -> Result<(), CliError> {
        let path = entry.path();
        if let Some(ext) = path.extension() {
            match ext.to_str() {
                Some("ts" | "js" | "jsx" | "tsx" | "svelte" | "vue") => {
                    println!("Scanning file for queries: {:?}", path);
                    let content = fs::read_to_string(path)?;
                    for cap in re.captures_iter(&content) {
                        if let Some(query) = cap.get(1) {
                            println!("Found inline query in {:?}", path);
                            self.analyze_query(query.as_str(), None)?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn load_schema(&mut self, path: &Path) -> Result<(), CliError> {
        println!("Loading schema from {:?}", path);

        if !path.exists() {
            println!("Path {:?} does not exist!", path);
            return Err(CliError::InvalidPath(path.to_path_buf()));
        }

        let mut files_processed = 0;

        if path.is_dir() {
            println!("Scanning directory {:?} for schema files...", path);
            for entry in WalkDir::new(path) {
                if self.process_entry(&entry?)? {
                    files_processed += 1;
                }
            }
        } else {
            println!("Processing single schema file: {:?}", path);
            self.load_schema_file(path)?;
            files_processed += 1;
        }

        if files_processed == 0 {
            println!("No schema files found!");
            return Err(CliError::NoSchemaFiles(path.to_path_buf()));
        }

        println!("Successfully processed {} schema file(s)", files_processed);
        Ok(())
    }

    fn load_schema_file(&mut self, path: &Path) -> Result<(), CliError> {
        println!("Reading schema file: {:?}", path);
        let content = fs::read_to_string(path)?;
        println!("Analyzing schema content...");
        surrealguard_core::analyzer::analyze(&mut self.ctx, &content)
            .map_err(CliError::Analysis)?;
        println!("Successfully analyzed schema file: {:?}", path);
        Ok(())
    }

    fn process_query_file(&mut self, path: &Path) -> Result<(), CliError> {
        println!("Processing query file: {:?}", path);
        let content = fs::read_to_string(path)?;

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("UnnamedQuery");

        println!("Processing query with name: {}", stem);
        let name = stem.to_string();
        self.analyze_query(&content, Some(name))?;
        println!("Successfully processed query file: {:?}", path);
        Ok(())
    }

    fn analyze_query(&mut self, query: &str, name: Option<String>) -> Result<(), CliError> {
        println!(
            "Analyzing query{}",
            name.as_ref()
                .map(|n| format!(": {}", n))
                .unwrap_or_default()
        );
        let kind = surrealguard_core::analyzer::analyze(&mut self.ctx, query)
            .map_err(CliError::Analysis)?;
        println!("Generated type kind, converting to TypeScript...");

        let type_def = surrealguard_codegen::typescript::TypeScriptGenerator::generate(&kind);
        let hash_full = Sha256::new().chain_update(query.as_bytes()).finalize();
        let hash = format!("{:x}", hash_full)[..8].to_string();

        let name = name.unwrap_or_else(|| format!("Query_{}", hash));
        let name = name
            .split('_')
            .map(|s| {
                let mut c = s.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().chain(c).collect(),
                }
            })
            .collect::<String>();

        // Format the Kind in a nice markdown comment.
        let doc_comment = format!(
            "/**\n * ## {} query results\n *\n * Kind:\n * ```\n * {}\n * ```\n */",
            name,
            kind.to_string()
        );

        let info = QueryInfo {
            name,
            query: query.to_string(),
            type_def,
            variables_type: None,
            doc_comment,
        };

        println!("Adding query to types map with query: {}", query);
        self.query_types.insert(hash, info);
        Ok(())
    }

    fn scan_source_files(&mut self, dirs: &[PathBuf]) -> Result<(), CliError> {
        println!("Scanning source files for inline queries...");
        let re = Regex::new(r#"surql`([^`]*)`"#).unwrap();

        for dir in dirs {
            println!("Scanning directory: {:?}", dir);
            for entry in WalkDir::new(dir) {
                self.process_source_entry(&entry?, &re)?;
            }
        }
        Ok(())
    }

    fn generate_output(&self, path: &Path, should_format: bool) -> Result<(), CliError> {
        println!("Generating output file: {:?}", path);

        let mut content = String::from(
            r#"import { type RecordId, Surreal } from 'surrealdb';

export type TypedResult<T extends keyof QueryMap> = QueryMap[T];

// Template literal tag helper for inline queries
export const surql = (
  strings: TemplateStringsArray,
  ...values: any[]
): keyof QueryMap => {
  return String.raw({ raw: strings }, ...values) as keyof QueryMap;
};

export type Queries = {"#,
        );

        // Generate Queries type for named queries.
        for info in self.query_types.values() {
            content.push_str(&format!(
                "\n    [{}]: {{ variables: {}, result: {}Result }};",
                info.name,
                info.variables_type.as_deref().unwrap_or("never"),
                info.name
            ));
        }
        content.push_str("\n}\n\n");

        // Generate QueryMap type for inline queries keyed by the exact query string.
        content.push_str("export type QueryMap = {\n");
        for info in self.query_types.values() {
            // Use JSON.stringify–style quoting to produce a valid string literal key.
            let key = serde_json::to_string(&info.query).unwrap();
            content.push_str(&format!("    {}: {}Result,\n", key, info.name));
        }
        content.push_str("}\n\n");

        // Generate each query's types and constants.
        for info in self.query_types.values() {
            content.push_str(&info.doc_comment);
            content.push('\n');
            content.push_str(&format!(
                "export const {} = `{}`;\n",
                info.name,
                info.query.replace("`", "\\`")
            ));
            content.push_str(&format!(
                "export type {}Result = [\n    {}\n];\n\n",
                info.name, info.type_def
            ));
        }

        // Generate the TypedSurreal class with both named and inline query support.
        content.push_str(r#"
export class TypedSurreal extends Surreal {
    // For named queries
    typed<Q extends keyof Queries>(query: Q, ...rest: Variables<Q>): Promise<Queries[Q]["result"]> {
        return this.query(query, rest[0])
    }

    // For inline queries
    async inline<T extends keyof QueryMap>(
        sql: T
    ): Promise<QueryMap[T]> {
        return super.query(sql) as Promise<QueryMap[T]>;
    }

    // Base query method remains unchanged for compatibility
    async query(sql: string | keyof Queries, vars?: any): Promise<any> {
        return super.query(sql, vars)
    }
}

export type Variables<Q extends keyof Queries> = Queries[Q]["variables"] extends never ? [] : [Queries[Q]["variables"]]
"#);
        println!("Writing output file...");
        fs::write(path, content)?;

        if should_format {
            println!("Formatting output with Prettier...");
            let output = std::process::Command::new("prettier")
                .arg("--write")
                .arg(path)
                .output()
                .map_err(|e| CliError::Template(format!("Failed to run Prettier: {}", e)))?;

            if !output.status.success() {
                let error = String::from_utf8_lossy(&output.stderr);
                return Err(CliError::Template(format!("Prettier failed: {}", error)));
            }
        }

        println!("Successfully wrote output file: {:?}", path);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), CliError> {
    let cli = Cli::parse();
    println!("CLI arguments: {:?}", cli);

    // Create the spinner using the provided template.
    let spinner_style = ProgressStyle::default_spinner()
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
        .template("{spinner:.green} {msg}")
        .map_err(|e| CliError::Template(e.to_string()))?;
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(spinner_style);
    spinner.enable_steady_tick(Duration::from_millis(100));

    let mut generator = Generator::new();

    spinner.set_message("Loading schema...");
    generator.load_schema(&cli.schema).map_err(|e| {
        spinner.finish_and_clear();
        e
    })?;

    if let Some(queries_path) = cli.queries.as_ref() {
        spinner.set_message(format!("Processing queries from {:?}", queries_path));
        if !queries_path.exists() {
            spinner.finish_and_clear();
            return Err(CliError::InvalidPath(queries_path.clone()));
        }

        if queries_path.is_dir() {
            let mut files_processed = 0;
            for entry_result in WalkDir::new(queries_path) {
                let entry = entry_result?;
                if entry.path().extension().map_or(false, |ext| ext == "surql") {
                    generator.process_query_file(entry.path())?;
                    files_processed += 1;
                }
            }
            if files_processed == 0 {
                spinner.finish_and_clear();
                return Err(CliError::NoQueryFiles(queries_path.clone()));
            }
        } else {
            generator.process_query_file(queries_path)?;
        }
    }

    if let Some(src_dirs) = cli.src.as_ref() {
        spinner.set_message("Scanning source directories for inline queries...");
        generator.scan_source_files(src_dirs)?;
    }

    spinner.set_message("Generating output...");
    generator.generate_output(&cli.output, cli.format)?;

    spinner.finish_and_clear();

    if cli.watch {
        println!("Watch mode not yet implemented");
    }

    println!("Done!");
    Ok(())
}
