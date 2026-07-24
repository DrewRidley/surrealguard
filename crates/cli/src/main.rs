//! The `surrealguard` CLI: analyzes a workspace of `.surql` sources and
//! reports findings as text or JSON, resolving severity through the
//! workspace's policy configuration (exit code reflects post-policy
//! errors).

mod render;

use clap::{Parser, Subcommand};
use serde::Serialize;
use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use surrealguard_diagnostics::{render_code, Severity};
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

    /// Generate the typed SurrealGuard client + query registry
    Generate {
        /// Output path for the generated module (default:
        /// surrealguard.generated.ts at the workspace root). It re-exports a
        /// runtime value, so the extension must be `.ts`, not `.d.ts`.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
}

const EXAMPLE_CONFIG: &str = r#"# surrealguard.toml — SurrealGuard workspace configuration.
# Docs: https://surrealguard.dev/docs/getting-started

[sources]
# Globs whose matches define the schema (DEFINE/REMOVE catalog effects).
schema = ["schema/**/*.surql", "migrations/**/*.surql"]
# Globs analyzed as queries against that schema.
queries = ["queries/**/*.surql", "src/**/*.surql"]
# Excluded from both sets.
ignore = ["target/**", "node_modules/**", ".git/**"]

[analysis]
# Tighten otherwise-advisory checks.
strict = false
# Target SurrealDB version for version-gated behavior.
surrealdb_version = "2"

[diagnostics]
# Promote every warning to an error (useful in CI).
warnings_as_errors = false
# Require a written reason on every inline suppression.
require_suppression_reasons = false

# Per-code lint levels: "allow" | "warn" | "deny" (or "error", an alias for
# "deny"). Keys are diagnostic codes (E1002, W7002, or the bare number 7002),
# whole-family wildcards ("7xxx" or "7*"), or the legacy named lints. A
# specific code always wins over a family wildcard that also covers it.
[lints]
# E1002 = "allow"   # silence a specific diagnostic (unknown field)
# "7xxx" = "warn"   # set every style lint (the 7-block) to warn
# W7002 = "allow"   # ...but silence the LET-shadowing lint
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
    /// rustc-style blocks for human output; empty when source text was
    /// never loaded (config errors).
    rendered: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct CheckDiagnostic {
    code: String,
    severity: &'static str,
    source: String,
    range: CheckRange,
    message: String,
    help: Vec<String>,
    related: Vec<CheckRelated>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct CheckRelated {
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
            help: Vec::new(),
            related: Vec::new(),
        }
    }
}

impl fmt::Display for CheckFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.rendered.is_empty() {
            write!(f, "SurrealGuard check failed")?;
            for diagnostic in &self.diagnostics {
                write!(f, "\n{diagnostic}")?;
            }
            return Ok(());
        }
        for block in &self.rendered {
            writeln!(f, "{block}")?;
        }
        write!(
            f,
            "check failed: {} error(s), {} diagnostic(s)",
            self.summary.errors, self.summary.diagnostics
        )
    }
}

impl Error for CheckFailed {}

fn run_check(start_dir: &Path) -> Result<CheckPassed, CheckFailed> {
    let root = find_workspace_root(start_dir);
    let config = load_workspace_config(&root).map_err(|error| CheckFailed {
        summary: CheckSummary {
            sources_checked: 0,
            diagnostics: 1,
            errors: 1,
        },
        diagnostics: vec![CheckDiagnostic::from_message(error.to_string())],
        rendered: Vec::new(),
    })?;
    let mut workspace = Workspace::new(config.clone());

    let mut source_texts: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
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
            rendered: Vec::new(),
        })?;
        let source_id = workspace.add_file_source(path, text.clone());
        source_texts.insert(source_id.to_string(), text);
    }

    let analysis = analyze_workspace(&workspace);

    // Findings carry their intrinsic class; presentation policy
    // (warnings-as-errors, per-code/family lint levels, suppression) applies
    // here, at the consumption edge — built once, shared with the LSP.
    let policy = config.policy();
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
                help: finding
                    .help()
                    .iter()
                    .map(|help| help.message.clone())
                    .collect(),
                related: finding
                    .related()
                    .iter()
                    .map(|related| CheckRelated {
                        source: related.span.source().to_string(),
                        range: CheckRange {
                            start: related.span.range().start(),
                            end: related.span.range().end(),
                        },
                        message: related.message.clone(),
                    })
                    .collect(),
            }
        })
        .collect();
    let rendered: Vec<String> = resolved
        .iter()
        .map(|(finding, severity)| render::render_finding(finding, *severity, &source_texts))
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
            rendered,
        })
    } else {
        Ok(CheckPassed { summary, rendered })
    }
}

#[derive(Debug)]
struct CheckPassed {
    summary: CheckSummary,
    /// Warning/hint blocks that survived policy on a clean run.
    rendered: Vec<String>,
}

/// Scans host sources for embedded queries, analyzes them against the
/// workspace schema, and writes the typed registry.
fn run_generate(root: &Path, out: Option<&Path>) -> Result<std::path::PathBuf, Box<dyn Error>> {
    let config = load_workspace_config(root)?;
    let mut workspace = surrealguard_workspace::analysis::Workspace::new(config.clone());
    for path in discover_surrealql_sources(root, &config) {
        let text = fs::read_to_string(&path)?;
        workspace.add_virtual_source(path.display().to_string(), text);
    }

    // Host files, honoring the same ignore patterns as .surql discovery.
    let host_paths: Vec<PathBuf> = {
        let mut paths: Vec<_> = WalkDir::new(root)
            .into_iter()
            .filter_entry(|entry| should_visit(entry, root, &config.sources.ignore))
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(DirEntry::into_path)
            .filter(|path| {
                matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("ts" | "tsx" | "js" | "jsx" | "svelte" | "vue" | "astro")
                )
            })
            .collect();
        paths.sort();
        paths
    };

    let mut queries = Vec::new();
    for path in host_paths {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for (index, query) in surrealguard_embed::extract(&path.display().to_string(), &text)
            .into_iter()
            .enumerate()
        {
            let source_id = workspace.add_virtual_source(
                format!("embedded://{}#{index}", path.display()),
                query.text.clone(),
            );
            queries.push((source_id, query));
        }
    }

    let analysis = analyze_workspace(&workspace);
    let entries: Vec<surrealguard_codegen::QueryEntry> = queries
        .iter()
        .filter_map(|(source_id, query)| {
            let output = analysis.sources.get(source_id)?;
            let result_type = output
                .response_kind
                .as_ref()
                .map_or_else(|| "unknown".into(), surrealguard_codegen::ts_type);
            Some(surrealguard_codegen::QueryEntry {
                parts: query.parts(),
                result_type,
                params: output.inferred_params.clone(),
            })
        })
        .collect();

    let out_path = out.map_or_else(|| root.join("surrealguard.generated.ts"), Path::to_path_buf);
    fs::write(&out_path, surrealguard_codegen::render_registry(&entries))?;
    Ok(out_path)
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

    // Schema sources are analyzed before query sources so their `DEFINE`s are
    // in scope for the queries that reference them. A file matching a `schema`
    // glob is schema; everything else is a query. Ties (a file matching both,
    // e.g. the default globs) resolve to schema — analyzing a definition early
    // is always safe.
    let (mut schema, mut queries): (Vec<PathBuf>, Vec<PathBuf>) = paths
        .into_iter()
        .partition(|path| matches_any_glob(root, path, &config.sources.schema));
    schema.append(&mut queries);
    schema
}

/// Whether `path` matches any of `globs`, evaluated relative to `root`.
fn matches_any_glob(root: &Path, path: &Path, globs: &[String]) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    globs
        .iter()
        .any(|glob| glob::Pattern::new(glob).is_ok_and(|pattern| pattern.matches_path(relative)))
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
            .is_some_and(|name| name == trimmed)
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
        Commands::Generate { out } => {
            let root = find_workspace_root(&env::current_dir()?);
            let written = run_generate(&root, out.as_deref())?;
            println!("Generated {}", written.display());
            Ok(())
        }
        Commands::Check { json } => {
            if !json {
                println!("Checking SurrealQL sources...");
            }
            match run_check(&env::current_dir()?) {
                Ok(passed) => {
                    if json {
                        println!(
                            "{}",
                            render_check_json(&passed.summary, &[])
                                .expect("json serialization should not fail")
                        );
                    } else {
                        for block in &passed.rendered {
                            println!("{block}");
                        }
                        println!(
                            "Checked {} source(s), found {} diagnostic(s)",
                            passed.summary.sources_checked, passed.summary.diagnostics
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
    fn example_config_matches_the_parsed_schema() {
        // The `init` template must parse against the real config schema, and
        // its sections must be the ones the parser actually reads — otherwise
        // `surrealguard init` would write a config the CLI silently ignores.
        let config =
            WorkspaceConfig::from_toml_str(EXAMPLE_CONFIG).expect("example config must parse");
        assert!(config
            .sources
            .schema
            .iter()
            .any(|glob| glob.contains("schema")));
        assert!(config
            .sources
            .queries
            .iter()
            .any(|glob| glob.contains("queries")));
        assert_eq!(config.analysis.surrealdb_version, "2");
    }

    #[test]
    fn schema_sources_are_analyzed_before_query_sources() {
        // A `schema/` file and a `queries/` file: the query must see the
        // schema even though "queries" sorts before "schema" by path. A
        // reference to a real table must NOT report `unknown table`, and a
        // bad field must report the precise `unknown field`.
        let root = temp_project_dir("schema-order");
        fs::create_dir_all(root.join("schema")).expect("schema dir");
        fs::create_dir_all(root.join("queries")).expect("queries dir");
        fs::write(
            root.join("surrealguard.toml"),
            "[sources]\nschema = [\"schema/**/*.surql\"]\nqueries = [\"queries/**/*.surql\"]\n",
        )
        .expect("write config");
        fs::write(
            root.join("schema/user.surql"),
            "DEFINE TABLE user SCHEMAFULL;\nDEFINE FIELD name ON user TYPE string;",
        )
        .expect("write schema");
        fs::write(root.join("queries/bad.surql"), "SELECT nope FROM user;").expect("write query");

        let failed = run_check(&root).expect_err("the unknown field should fail the check");
        let codes: Vec<&str> = failed.diagnostics.iter().map(|d| d.code.as_str()).collect();
        assert!(
            codes.iter().any(|c| c.starts_with("E1002")),
            "expected unknown-field E1002, got {codes:?}"
        );
        assert!(
            !codes.iter().any(|c| c.starts_with("E1001")),
            "schema was not applied before the query — spurious unknown-table: {codes:?}"
        );
    }

    #[test]
    fn check_loads_surrealql_files_through_workspace_analysis() {
        let root = temp_project_dir("valid-check");
        fs::create_dir_all(root.join("schema")).expect("create schema dir");
        fs::write(root.join("surrealguard.toml"), "").expect("write config");
        fs::write(root.join("schema/person.surql"), "DEFINE TABLE person;").expect("write schema");

        let summary = run_check(&root).expect("valid workspace should check");

        assert_eq!(summary.summary.sources_checked, 1);
        assert_eq!(summary.summary.diagnostics, 0);
        assert_eq!(summary.summary.errors, 0);
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

        assert_eq!(summary.summary.sources_checked, 1);
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

    #[test]
    fn warnings_as_errors_config_fails_the_check_on_a_warning_finding() {
        // A DROP table with a declared field yields only the 4022 warning
        // (SELECT from a DROP table) with no lint noise.
        let root = temp_project_dir("warn-as-error");
        fs::write(
            root.join("surrealguard.toml"),
            "[diagnostics]\nwarnings_as_errors = true\n",
        )
        .expect("write config");
        fs::write(
            root.join("schema.surql"),
            "DEFINE TABLE t DROP SCHEMAFULL;\nDEFINE FIELD x ON t TYPE int;\nSELECT * FROM t;",
        )
        .expect("write source");

        let err = run_check(&root).expect_err("promoted warning should fail the check");

        assert_eq!(err.summary.errors, 1);
        assert!(err
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "E4022" && diagnostic.severity == "error"));
    }

    #[test]
    fn warning_only_source_passes_without_warnings_as_errors() {
        let root = temp_project_dir("warn-only-clean");
        fs::write(root.join("surrealguard.toml"), "").expect("write config");
        fs::write(
            root.join("schema.surql"),
            "DEFINE TABLE t DROP SCHEMAFULL;\nDEFINE FIELD x ON t TYPE int;\nSELECT * FROM t;",
        )
        .expect("write source");

        // The 4022 warning is reported but keeps the check clean: it counts
        // as a diagnostic, not an error.
        let summary = run_check(&root).expect("warning-only source should pass");
        assert_eq!(summary.summary.diagnostics, 1);
        assert_eq!(summary.summary.errors, 0);
    }

    #[test]
    fn error_finding_fails_the_check_without_any_policy() {
        let root = temp_project_dir("error-fails");
        fs::write(root.join("surrealguard.toml"), "").expect("write config");
        // An unknown table is an error-class finding (E1001).
        fs::write(root.join("query.surql"), "SELECT * FROM ghost;").expect("write source");

        let err = run_check(&root).expect_err("error finding should fail the check");

        assert!(err.summary.errors >= 1);
        assert!(err
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "E1001" && diagnostic.severity == "error"));
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
