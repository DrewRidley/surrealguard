use clap::{Parser, Subcommand};
use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use surrealguard_codegen::{self, CodegenError, Config};
use surrealguard_diagnostics::Severity;
use surrealguard_workspace::config::WorkspaceConfig;
use surrealguard_workspace::{analyze_workspace, Workspace};
use walkdir::{DirEntry, WalkDir};

#[derive(Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new surrealguard.toml config file
    Init,

    /// Check schema and queries without generating output
    Check,

    /// Generate code once and exit
    Run,

    /// Generate code and watch for changes
    Watch,
}

const EXAMPLE_CONFIG: &str = r#"version = "1.0"
language = "typescript"

[schema]
path = "schema/"

[queries]
path = "queries/"
src = ["src/"]

[output]
path = "src/queries.ts"
format = true
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
struct CheckSummary {
    sources_checked: usize,
    diagnostics: usize,
    errors: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CheckFailed {
    diagnostics: Vec<String>,
}

impl fmt::Display for CheckFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SurrealGuard check failed")?;
        for diagnostic in &self.diagnostics {
            write!(f, "\n{diagnostic}")?;
        }
        Ok(())
    }
}

impl Error for CheckFailed {}

fn run_check(start_dir: &Path) -> Result<CheckSummary, CheckFailed> {
    let root = find_workspace_root(start_dir);
    let config = load_workspace_config(&root).map_err(|error| CheckFailed {
        diagnostics: vec![error.to_string()],
    })?;
    let mut workspace = Workspace::new(config.clone());

    for path in discover_surrealql_sources(&root, &config) {
        let text = fs::read_to_string(&path).map_err(|error| CheckFailed {
            diagnostics: vec![format!("{}: {error}", path.display())],
        })?;
        workspace.add_file_source(path, text);
    }

    let analysis = analyze_workspace(&workspace);
    let diagnostics: Vec<_> = analysis
        .diagnostics
        .iter()
        .map(|finding| {
            format!(
                "{} {}: {}",
                finding.code(),
                finding.span().source(),
                finding.message()
            )
        })
        .collect();
    let errors = analysis
        .diagnostics
        .iter()
        .filter(|finding| finding.effective_severity() == Severity::Error)
        .count();
    let summary = CheckSummary {
        sources_checked: analysis.sources.len(),
        diagnostics: analysis.diagnostics.len(),
        errors,
    };

    if errors > 0 {
        Err(CheckFailed { diagnostics })
    } else {
        Ok(summary)
    }
}

fn find_workspace_root(start_dir: &Path) -> PathBuf {
    let mut current = start_dir.to_path_buf();
    loop {
        if current.join("surrealguard.toml").exists() {
            return current;
        }
        if !current.pop() {
            return start_dir.to_path_buf();
        }
    }
}

fn load_workspace_config(root: &Path) -> Result<WorkspaceConfig, Box<dyn Error>> {
    let config_path = root.join("surrealguard.toml");
    if config_path.exists() {
        let text = fs::read_to_string(config_path)?;
        Ok(WorkspaceConfig::from_toml_str(&text)?)
    } else {
        Ok(WorkspaceConfig::default())
    }
}

fn discover_surrealql_sources(root: &Path, config: &WorkspaceConfig) -> Vec<PathBuf> {
    let mut paths: Vec<_> = WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| should_visit(entry, root, &config.sources.ignore))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(DirEntry::into_path)
        .filter(|path| is_surrealql_source(path))
        .collect();
    paths.sort();
    paths
}

fn should_visit(entry: &DirEntry, root: &Path, ignore_patterns: &[String]) -> bool {
    if entry.path() == root {
        return true;
    }

    let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
    !ignore_patterns
        .iter()
        .any(|pattern| matches_simple_ignore(relative, pattern))
}

fn matches_simple_ignore(relative: &Path, pattern: &str) -> bool {
    let trimmed = pattern.strip_suffix("/**").unwrap_or(pattern);
    relative.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .map(|name| name == trimmed)
            .unwrap_or(false)
    })
}

fn is_surrealql_source(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("surql" | "surrealql")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn check_loads_surrealql_files_through_workspace_analysis() {
        let root = temp_project_dir("valid-check");
        fs::create_dir_all(root.join("schema")).expect("create schema dir");
        fs::write(root.join("surrealguard.toml"), "").expect("write config");
        fs::write(root.join("schema/person.surql"), "DEFINE TABLE person;").expect("write schema");

        let summary = run_check(&root).expect("valid workspace should check");

        assert_eq!(summary.sources_checked, 1);
        assert_eq!(summary.diagnostics, 0);
        assert_eq!(summary.errors, 0);
    }

    #[test]
    fn check_reports_syntax_errors_from_workspace_analysis() {
        let root = temp_project_dir("invalid-check");
        fs::create_dir_all(root.join("queries")).expect("create queries dir");
        fs::write(root.join("surrealguard.toml"), "").expect("write config");
        fs::write(root.join("queries/bad.surql"), "SELECT * FROM ;").expect("write query");

        let err = run_check(&root).expect_err("syntax diagnostics should fail check");

        assert!(err.to_string().contains("S0001"));
        assert!(err.to_string().contains("bad.surql"));
    }

    #[test]
    fn check_finds_config_from_parent_directory() {
        let root = temp_project_dir("parent-config-check");
        let child = root.join("nested").join("project");
        fs::create_dir_all(&child).expect("create child dir");
        fs::write(root.join("surrealguard.toml"), "").expect("write config");
        fs::write(root.join("person.surql"), "DEFINE TABLE person;").expect("write schema");

        let summary = run_check(&child).expect("valid parent workspace should check");

        assert_eq!(summary.sources_checked, 1);
    }

    fn temp_project_dir(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time is after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("surrealguard-{name}-{unique}"));
        fs::create_dir_all(&root).expect("create temp project root");
        root
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => {
            let config_path = env::current_dir()?.join("surrealguard.toml");
            if config_path.exists() {
                println!("Config file already exists at {}", config_path.display());
                return Ok(());
            }

            fs::write(&config_path, EXAMPLE_CONFIG)?;
            println!("Created surrealguard.toml");
            Ok(())
        }
        Commands::Check => {
            println!("Checking SurrealQL sources...");
            match run_check(&env::current_dir()?) {
                Ok(summary) => {
                    println!(
                        "Checked {} source(s), found {} diagnostic(s)",
                        summary.sources_checked, summary.diagnostics
                    );
                    println!("All checks passed!");
                    Ok(())
                }
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            }
        }
        cmd => {
            match Config::find_and_load(&env::current_dir()?) {
                Ok((config, config_dir)) => {
                    env::set_current_dir(&config_dir)?;
                    println!("Using configuration from: {}", config_dir.display());

                    match cmd {
                        Commands::Run => {
                            println!("Generating code...");
                            surrealguard_codegen::generate(&config)?;
                            println!("Done!");
                        }
                        Commands::Watch => {
                            println!("Starting watch mode...");
                            surrealguard_codegen::watch(&config)?;
                        }
                        Commands::Init | Commands::Check => unreachable!(),
                    }
                    Ok(())
                }
                Err(CodegenError::ConfigNotFound(_)) => {
                    eprintln!("Error: No surrealguard.toml found in current directory or parent directories");
                    eprintln!("Run 'surrealguard init' to create a new config file");
                    std::process::exit(1);
                }
                Err(e) => Err(e.into()),
            }
        }
    }
}
