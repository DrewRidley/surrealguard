//! The `surrealguard` CLI: analyzes a workspace of `.surql` sources and
//! reports findings as text or JSON, resolving severity through the
//! workspace's policy configuration (exit code reflects post-policy
//! errors).

use clap::{Parser, Subcommand};
use serde::Serialize;
use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use surrealguard_diagnostics::{render_code, PolicyConfig, Severity};
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
    Check {
        /// Emit machine-readable JSON diagnostics
        #[arg(long)]
        json: bool,
    },
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct CheckSummary {
    sources_checked: usize,
    diagnostics: usize,
    errors: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CheckFailed {
    summary: CheckSummary,
    diagnostics: Vec<CheckDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct CheckDiagnostic {
    code: String,
    severity: &'static str,
    source: String,
    range: CheckRange,
    message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
struct CheckRange {
    start: u32,
    end: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct CheckJson<'a> {
    summary: &'a CheckSummary,
    diagnostics: &'a [CheckDiagnostic],
}

impl fmt::Display for CheckDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}: {}", self.code, self.source, self.message)
    }
}

impl CheckDiagnostic {
    fn from_message(message: String) -> Self {
        Self {
            code: "S0000".into(),
            severity: "error",
            source: "".into(),
            range: CheckRange { start: 0, end: 0 },
            message,
        }
    }
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
        summary: CheckSummary {
            sources_checked: 0,
            diagnostics: 1,
            errors: 1,
        },
        diagnostics: vec![CheckDiagnostic::from_message(error.to_string())],
    })?;
    let mut workspace = Workspace::new(config.clone());

    for path in discover_surrealql_sources(&root, &config) {
        let text = fs::read_to_string(&path).map_err(|error| CheckFailed {
            summary: CheckSummary {
                sources_checked: 0,
                diagnostics: 1,
                errors: 1,
            },
            diagnostics: vec![CheckDiagnostic::from_message(format!(
                "{}: {error}",
                path.display()
            ))],
        })?;
        workspace.add_file_source(path, text);
    }

    let analysis = analyze_workspace(&workspace);

    // Findings carry their intrinsic class; presentation policy
    // (warnings-as-errors, lint levels, suppression) applies here, at the
    // consumption edge.
    let mut policy = PolicyConfig::default();
    policy.set_warnings_as_errors(config.diagnostics.warnings_as_errors);
    let resolved: Vec<_> = analysis
        .diagnostics
        .iter()
        .filter_map(|finding| {
            policy
                .resolve_severity(finding.code(), finding.severity())
                .map(|severity| (finding, severity))
        })
        .collect();

    let diagnostics: Vec<_> = resolved
        .iter()
        .map(|(finding, severity)| {
            let range = finding.span().range();
            CheckDiagnostic {
                code: render_code(finding.code(), *severity),
                severity: severity_name(*severity),
                source: finding.span().source().to_string(),
                range: CheckRange {
                    start: range.start(),
                    end: range.end(),
                },
                message: finding.message().to_string(),
            }
        })
        .collect();
    let errors = resolved
        .iter()
        .filter(|(_, severity)| *severity == Severity::Error)
        .count();
    let summary = CheckSummary {
        sources_checked: analysis.sources.len(),
        diagnostics: resolved.len(),
        errors,
    };

    if errors > 0 {
        Err(CheckFailed {
            summary,
            diagnostics,
        })
    } else {
        Ok(summary)
    }
}

fn render_check_json(
    summary: &CheckSummary,
    diagnostics: &[CheckDiagnostic],
) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&CheckJson {
        summary,
        diagnostics,
    })
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Hint => "hint",
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
        Commands::Check { json } => {
            if !json {
                println!("Checking SurrealQL sources...");
            }
            match run_check(&env::current_dir()?) {
                Ok(summary) => {
                    if json {
                        println!(
                            "{}",
                            render_check_json(&summary, &[])
                                .expect("json serialization should not fail")
                        );
                    } else {
                        println!(
                            "Checked {} source(s), found {} diagnostic(s)",
                            summary.sources_checked, summary.diagnostics
                        );
                        println!("All checks passed!");
                    }
                    Ok(())
                }
                Err(error) => {
                    if json {
                        println!(
                            "{}",
                            render_check_json(&error.summary, &error.diagnostics)
                                .expect("json serialization should not fail")
                        );
                    } else {
                        eprintln!("{error}");
                    }
                    std::process::exit(1);
                }
            }
        }
    }
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

    #[test]
    fn check_json_output_uses_stable_diagnostic_keys() {
        let root = temp_project_dir("json-check");
        fs::create_dir_all(root.join("queries")).expect("create queries dir");
        fs::write(root.join("surrealguard.toml"), "").expect("write config");
        fs::write(root.join("queries/bad.surql"), "SELECT * FROM ;").expect("write query");

        let err = run_check(&root).expect_err("syntax diagnostics should fail check");
        let json = render_check_json(&err.summary, &err.diagnostics).expect("json renders");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");

        assert_eq!(value["summary"]["sources_checked"], 1);
        assert_eq!(value["summary"]["diagnostics"], 1);
        assert_eq!(value["summary"]["errors"], 1);
        assert_eq!(value["diagnostics"][0]["code"], "S0001");
        assert_eq!(value["diagnostics"][0]["severity"], "error");
        assert!(value["diagnostics"][0]["source"]
            .as_str()
            .unwrap()
            .contains("bad.surql"));
        assert_eq!(value["diagnostics"][0]["message"], "SurrealQL syntax error");
        assert!(value["diagnostics"][0]["range"]["start"].is_number());
        assert!(value["diagnostics"][0]["range"]["end"].is_number());
    }

    #[test]
    fn check_accepts_json_flag() {
        let cli = Cli::try_parse_from(["surrealguard", "check", "--json"]).expect("cli parses");

        match cli.command {
            Commands::Check { json } => assert!(json),
            _ => panic!("expected check command"),
        }
    }

    #[test]
    fn codegen_commands_are_not_part_of_the_rewrite_cli() {
        assert!(Cli::try_parse_from(["surrealguard", "run"]).is_err());
        assert!(Cli::try_parse_from(["surrealguard", "watch"]).is_err());
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
