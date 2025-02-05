use clap::Parser;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::{WalkDir, DirEntry};
use sha2::{Sha256, Digest};
use regex::Regex;
use serde::Serialize;
use std::error::Error as StdError;

use surrealguard_core::analyzer::{analyze, context::AnalyzerContext};
use surrealguard_codegen::typescript::TypeScriptGenerator;

#[derive(thiserror::Error, Debug)]
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
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(long, value_name = "PATH")]
    schema: PathBuf,

    #[arg(long, value_name = "PATH")]
    queries: Option<PathBuf>,

    #[arg(long, value_name = "DIR")]
    src: Option<Vec<PathBuf>>,

    #[arg(long, default_value = "src/queries.ts")]
    output: PathBuf,

    #[arg(long)]
    watch: bool,
}

#[derive(Debug, Clone, Serialize)]
struct QueryInfo {
    name: String,
    query: String,
    type_def: String,
    variables_type: Option<String>,
    doc_comment: String,
}

struct Generator {
    ctx: AnalyzerContext,
    query_types: HashMap<String, QueryInfo>,
}

impl Generator {
    fn new() -> Self {
        println!("Initializing Generator...");
        Self {
            ctx: AnalyzerContext::new(),
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

        println!("Successfully processed {} schema files", files_processed);
        Ok(())
    }

    fn load_schema_file(&mut self, path: &Path) -> Result<(), CliError> {
        println!("Reading schema file: {:?}", path);
        let content = fs::read_to_string(path)?;
        println!("Analyzing schema content...");
        analyze(&mut self.ctx, &content).map_err(CliError::Analysis)?;
        println!("Successfully analyzed schema file: {:?}", path);
        Ok(())
    }

    fn process_query_file(&mut self, path: &Path) -> Result<(), CliError> {
        println!("Processing query file: {:?}", path);
        let content = fs::read_to_string(path)?;

        let stem = path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("UnnamedQuery");

        println!("Processing query with name: {}", stem);
        let name = stem.to_string();
        self.analyze_query(&content, Some(name))?;
        println!("Successfully processed query file: {:?}", path);
        Ok(())
    }

    fn analyze_query(&mut self, query: &str, name: Option<String>) -> Result<(), CliError> {
        println!("Analyzing query{}", name.as_ref().map(|n| format!(": {}", n)).unwrap_or_default());

        let kind = analyze(&mut self.ctx, query).map_err(CliError::Analysis)?;
        println!("Generated type kind, converting to TypeScript...");

        let type_def = TypeScriptGenerator::generate(&kind);
        // (Keep using the hash for naming purposes, if desired.)
        let hash = format!("{:x}", Sha256::new()
            .chain_update(query.as_bytes())
            .finalize())[..8].to_string();

        let name = name.unwrap_or_else(|| format!("Query_{}", hash));
        let name = name.split('_')
            .map(|s| {
                let mut c = s.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().chain(c).collect(),
                }
            })
            .collect::<String>();

        let doc_comment = format!(
            "/**\n * ## {} query results:\n *\n * ```surql\n * /// -------------\n * /// Result:\n * /// -------------\n * {}\n * ```\n */",
            name,
            type_def.replace("\n", "\n * ")
        );

        let info = QueryInfo {
            name,
            query: query.to_string(), // <-- Use the query string itself
            type_def,
            variables_type: None,
            doc_comment,
        };

        println!("Adding query to types map (for inline lookup) with query: {}", query);
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

    fn generate_output(&self, path: &Path) -> Result<(), CliError> {
        println!("Generating output file: {:?}", path);

        let mut content = String::from(
            "import { type RecordId, Surreal } from 'surrealdb'\n\n"
        );

        // Generate Queries type for named queries (unchanged)
        content.push_str("export type Queries = {\n");
        for info in self.query_types.values() {
            content.push_str(&format!(
                "    [{}]: {{ variables: {}, result: {}Result }}\n",
                info.name,
                info.variables_type.as_deref().unwrap_or("never"),
                info.name
            ));
        }
        content.push_str("}\n\n");

        // Generate QueryMap type for inline queries keyed by the exact query string.
        content.push_str("export type QueryMap = {\n");
        for info in self.query_types.values() {
            // Use JSON.stringify style quoting to produce a valid string literal key.
            let key = serde_json::to_string(&info.query).unwrap();
            content.push_str(&format!(
                "    {}: {}Result,\n",
                key,
                info.name
            ));
        }
        content.push_str("}\n\n");

        // Generate each query's types and constants (unchanged)
        for info in self.query_types.values() {
            content.push_str(&info.doc_comment);
            content.push('\n');
            content.push_str(&format!(
                "export const {} = `{}`\n",
                info.name,
                info.query.replace("`", "\\`")
            ));
            content.push_str(&format!(
                "export type {}Result = [\n    {}\n]\n\n",
                info.name,
                info.type_def
            ));
        }

        // Generate TypedSurreal class with both named and inline query support.
        // Notice the inline method now uses QueryMap.
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
        println!("Successfully wrote output file: {:?}", path);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn StdError + Send + Sync>> {
    let cli = Cli::parse();
    println!("CLI arguments: {:?}", cli);

    let mut generator = Generator::new();

    // Load schema
    println!("Loading schema...");
    generator.load_schema(&cli.schema).map_err(|e| Box::new(e) as Box<dyn StdError + Send + Sync>)?;

    // Process query files if specified
    if let Some(queries_path) = cli.queries.as_ref() {
        println!("Processing queries from: {:?}", queries_path);
        if !queries_path.exists() {
            return Err(Box::new(CliError::InvalidPath(queries_path.clone())) as Box<dyn StdError + Send + Sync>);
        }

        if queries_path.is_dir() {
            let mut files_processed = 0;
            for entry_result in WalkDir::new(queries_path) {
                match entry_result {
                    Ok(entry) => {
                        if entry.path().extension().map_or(false, |ext| ext == "surql") {
                            generator.process_query_file(entry.path())
                                .map_err(|e| Box::new(e) as Box<dyn StdError + Send + Sync>)?;
                            files_processed += 1;
                        }
                    },
                    Err(e) => return Err(Box::new(CliError::Walk(e)) as Box<dyn StdError + Send + Sync>),
                }
            }
            if files_processed == 0 {
                return Err(Box::new(CliError::NoQueryFiles(queries_path.clone())) as Box<dyn StdError + Send + Sync>);
            }
        } else {
            generator.process_query_file(queries_path)
                .map_err(|e| Box::new(e) as Box<dyn StdError + Send + Sync>)?;
        }
    }

    // Scan source files for inline queries if specified
    if let Some(src_dirs) = cli.src.as_ref() {
        println!("Scanning source directories: {:?}", src_dirs);
        generator.scan_source_files(src_dirs)
            .map_err(|e| Box::new(e) as Box<dyn StdError + Send + Sync>)?;
    }

    // Generate the output file
    println!("Generating output...");
    generator.generate_output(&cli.output)
        .map_err(|e| Box::new(e) as Box<dyn StdError + Send + Sync>)?;

    if cli.watch {
        println!("Watch mode not yet implemented");
    }

    println!("Done!");
    Ok(())
}
