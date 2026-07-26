//! The `surrealguard` command-line interface.
//!
//! `surrealguard` analyzes a workspace of `.surql` sources — and the SurrealQL
//! embedded in host-language files — reporting findings as rustc-style text or
//! machine-readable JSON. Effective severity is resolved through the
//! workspace's policy configuration, and the process exit code reflects the
//! post-policy error count, so `surrealguard check` drops straight into CI.
//!
//! # Subcommands
//!
//! - `surrealguard init` — write a starter `surrealguard.toml` to the current
//!   directory.
//! - `surrealguard check [--json] [--watch]` — discover sources via the config
//!   globs, split them into the schema set (DEFINE/REMOVE catalog) and the
//!   query set, run the analyzer, and print findings. Exits non-zero when any
//!   survive as errors.
//! - `surrealguard generate [--out PATH] [--watch]` — emit the typed TypeScript
//!   client and literal-keyed query registry (defaults to
//!   `surrealguard.generated.ts` at the workspace root).
//!
//! `--watch` turns either verb into a loop: run once, then re-run on every
//! change to an input the analysis consumes (`.surql` sources, host files
//! carrying embedded queries, and `surrealguard.toml`). See [`watch`].
//!
//! # `surrealguard.toml`
//!
//! The config file — discovered by walking up from the working directory —
//! declares the source globs (`[sources]` `schema` / `queries` / `ignore`),
//! analysis toggles (`[analysis]` `strict`, `surrealdb_version`), diagnostic
//! policy (`[diagnostics]` `warnings_as_errors`, `require_suppression_reasons`),
//! and per-code lint levels (`[lints]`). `surrealguard init` writes a fully
//! commented example.

mod render;
mod watch;

use clap::{Parser, Subcommand};
use serde::Serialize;
use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use surrealguard_diagnostics::{render_code, Finding, Severity};
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};
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

        /// Re-check on every change to a `.surql` source, a host file, or
        /// `surrealguard.toml`. Runs once first, then blocks until interrupted.
        ///
        /// Mutually exclusive with `--json`: that flag is a one-run machine
        /// contract (one document, one exit code), and a watch produces neither.
        #[arg(long, conflicts_with = "json")]
        watch: bool,
    },

    /// Generate the typed SurrealGuard client + query registry
    Generate {
        /// Output path for the generated module (default:
        /// surrealguard.generated.ts at the workspace root). It re-exports a
        /// runtime value, so the extension must be `.ts`, not `.d.ts`.
        #[arg(long)]
        out: Option<std::path::PathBuf>,

        /// Regenerate on every change to a `.surql` source, a host file, or
        /// `surrealguard.toml`. Runs once first, then blocks until interrupted.
        #[arg(long)]
        watch: bool,
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

    // Embedded queries in host files (`surql` tagged templates in .ts/.svelte/…)
    // are part of the workspace: they are the queries the client actually runs.
    // `generate` has always analyzed them, so a `check` that ignored them would
    // pass a workspace whose `generate` then fails with errors — CI green, build
    // broken. Their findings are remapped to `host_file:line` below.
    let mut embedded: std::collections::BTreeMap<String, (surrealguard_embed::EmbeddedQuery, String)> =
        std::collections::BTreeMap::new();
    for path in discover_host_sources(&root, &config) {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let host_id = path.display().to_string();
        let queries = surrealguard_embed::extract(&host_id, &text);
        if queries.is_empty() {
            continue;
        }
        for (index, query) in queries.into_iter().enumerate() {
            let source_id = workspace
                .add_virtual_source(format!("embedded://{host_id}#{index}"), query.text.clone());
            embedded.insert(source_id.to_string(), (query, host_id.clone()));
        }
        source_texts.insert(host_id, text);
    }

    let analysis = analyze_workspace(&workspace);

    // Findings carry their intrinsic class; presentation policy
    // (warnings-as-errors, per-code/family lint levels, suppression) applies
    // here, at the consumption edge — built once, shared with the LSP.
    let policy = config.policy();
    let resolved: Vec<(Finding, Severity)> = analysis
        .diagnostics
        .iter()
        .filter_map(|finding| {
            let severity = policy.resolve_severity(finding.code(), finding.severity())?;
            // A finding raised on an embedded query carries `embedded://host#n`
            // coordinates, which mean nothing to the user. Rewrite it onto the
            // host file so it reads as `app.ts:12:5`, exactly as `generate` does.
            let finding = match embedded.get(&finding.span().source().to_string()) {
                Some((query, host_id)) => {
                    remap_finding_to_host(finding, query, finding.span().source(), host_id)
                }
                None => finding.clone(),
            };
            Some((finding, severity))
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
        Ok(CheckPassed {
            summary,
            diagnostics,
            rendered,
        })
    }
}

#[derive(Debug)]
struct CheckPassed {
    summary: CheckSummary,
    /// The structured warning/hint findings that survived policy on a clean run.
    /// A clean run is not a silent run: `--json` consumers need these, and the
    /// summary already counts them, so omitting them contradicted the summary.
    diagnostics: Vec<CheckDiagnostic>,
    /// Warning/hint blocks that survived policy on a clean run.
    rendered: Vec<String>,
}

/// A successful `generate`: the registry path and any warning/hint blocks
/// (host-mapped) that survived policy on the clean run.
#[derive(Debug)]
struct GenerateReport {
    path: std::path::PathBuf,
    warnings: Vec<String>,
    /// How many embedded queries landed in the registry. `--watch` prints it so
    /// a repeating line still shows the run did something.
    queries: usize,
}

/// `generate` refused to write because an embedded query has an error-severity
/// finding. Carries every finding rendered at its real `host_file:line` so the
/// user can fix the query the client actually runs.
#[derive(Debug)]
struct GenerateFailed {
    /// rustc-style blocks for every finding on the failing run (errors first,
    /// then any warnings/hints), host-mapped to `file:line`.
    rendered: Vec<String>,
    errors: usize,
}

impl fmt::Display for GenerateFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for block in &self.rendered {
            writeln!(f, "{block}")?;
        }
        write!(
            f,
            "generate failed: {} error(s) in embedded queries — registry not written",
            self.errors
        )
    }
}

impl Error for GenerateFailed {}

/// Rebuilds a finding whose span is in embedded-query coordinates so it points
/// at the host file: spans belonging to the embedded query (`embed_source`) are
/// mapped through its segment map and re-sourced to `host_id` (so rendering
/// resolves `host_file:line`); spans in other sources (e.g. a schema file the
/// query references) are left as-is.
fn remap_finding_to_host(
    finding: &Finding,
    query: &surrealguard_embed::EmbeddedQuery,
    embed_source: &SourceId,
    host_id: &str,
) -> Finding {
    let host_sid = SourceId::new(host_id);
    let map_span = |span: &SourceSpan| -> SourceSpan {
        if span.source() != embed_source {
            return span.clone();
        }
        let range = span.range();
        let mapped = query.host_span(range.start() as usize..range.end() as usize);
        let byte_range = ByteRange::new(mapped.start as u32, mapped.end as u32)
            .unwrap_or_else(|_| ByteRange::new(0, 1).expect("0..1 is ordered"));
        SourceSpan::new(host_sid.clone(), byte_range)
    };

    let mut rebuilt = Finding::new(
        map_span(finding.span()),
        finding.code(),
        finding.severity(),
        finding.message(),
    );
    for help in finding.help() {
        rebuilt = rebuilt.with_help(help.message.clone());
    }
    for related in finding.related() {
        rebuilt = rebuilt.with_related(map_span(&related.span), related.message.clone());
    }
    rebuilt
}

/// Scans host sources for embedded queries, analyzes them against the
/// workspace schema, and writes the typed registry. Findings raised on the
/// embedded queries are reported at their host `file:line`: warnings/hints are
/// printed but do not block generation, while any error-severity finding
/// aborts before writing so a broken registry never overwrites a good one.
fn run_generate(root: &Path, out: Option<&Path>) -> Result<GenerateReport, Box<dyn Error>> {
    let config = load_workspace_config(root)?;
    let mut workspace = surrealguard_workspace::analysis::Workspace::new(config.clone());
    for path in discover_surrealql_sources(root, &config) {
        let text = fs::read_to_string(&path)?;
        workspace.add_virtual_source(path.display().to_string(), text);
    }

    // Host files, honoring the same ignore patterns as .surql discovery.
    let host_paths = discover_host_sources(root, &config);

    // Retain each host file's text so findings render against real source.
    let mut host_texts: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    let mut queries = Vec::new();
    for path in host_paths {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let host_id = path.display().to_string();
        let embedded = surrealguard_embed::extract(&host_id, &text);
        if embedded.is_empty() {
            continue;
        }
        for (index, query) in embedded.into_iter().enumerate() {
            let source_id = workspace
                .add_virtual_source(format!("embedded://{host_id}#{index}"), query.text.clone());
            queries.push((source_id, query, host_id.clone()));
        }
        host_texts.insert(host_id, text);
    }

    let analysis = analyze_workspace(&workspace);

    // Map each embedded query's findings back onto its host file, resolving
    // presentation severity through the same policy the check path uses.
    let policy = config.policy();
    let mut rendered_errors = Vec::new();
    let mut rendered_warnings = Vec::new();
    for (source_id, query, host_id) in &queries {
        let Some(output) = analysis.sources.get(source_id) else {
            continue;
        };
        for finding in &output.diagnostics {
            let Some(severity) = policy.resolve_severity(finding.code(), finding.severity()) else {
                continue;
            };
            let host_finding = remap_finding_to_host(finding, query, source_id, host_id);
            let block = render::render_finding(&host_finding, severity, &host_texts);
            if severity == Severity::Error {
                rendered_errors.push(block);
            } else {
                rendered_warnings.push(block);
            }
        }
    }

    if !rendered_errors.is_empty() {
        // Don't overwrite a good registry with a broken one — bail before writing.
        let errors = rendered_errors.len();
        let mut rendered = rendered_errors;
        rendered.extend(rendered_warnings);
        return Err(Box::new(GenerateFailed { rendered, errors }));
    }

    let entries: Vec<surrealguard_codegen::QueryEntry> = queries
        .iter()
        .filter_map(|(source_id, query, _host_id)| {
            let output = analysis.sources.get(source_id)?;
            // The SurrealDB SDK returns one result per statement, in order. Build
            // the per-statement response tuple: a responding statement contributes
            // its rendered result kind, a non-responder contributes `null`.
            let elements: Vec<String> = output
                .statements
                .iter()
                .map(|statement| {
                    statement
                        .response_kind
                        .as_ref()
                        .map_or_else(|| "null".into(), surrealguard_codegen::ts_type)
                })
                .collect();
            let result_type = format!("[{}]", elements.join(", "));
            Some(surrealguard_codegen::QueryEntry {
                parts: query.parts(),
                result_type,
                params: output.inferred_params.clone(),
            })
        })
        .collect();

    let out_path = generated_registry_path(root, out);
    fs::write(&out_path, surrealguard_codegen::render_registry(&entries))?;
    Ok(GenerateReport {
        path: out_path,
        warnings: rendered_warnings,
        queries: entries.len(),
    })
}

/// One watched `generate` run, reduced to the log line `--watch` prints.
///
/// A failure is reported and returned, never propagated: a transient error —
/// the syntax error you are halfway through typing — must not end the watch.
fn generate_outcome(root: &Path, out: Option<&Path>) -> watch::RunOutcome {
    match run_generate(root, out) {
        Ok(report) => watch::RunOutcome {
            ok: true,
            summary: format!(
                "wrote {} ({} quer{})",
                report
                    .path
                    .strip_prefix(root)
                    .unwrap_or(&report.path)
                    .display(),
                report.queries,
                if report.queries == 1 { "y" } else { "ies" }
            ),
            detail: report.warnings.concat(),
        },
        // The findings already say everything the trailing "generate failed:"
        // line would, and the run line above carries the count — so print the
        // blocks only, and keep the line short enough to scan when it repeats.
        Err(error) => match error.downcast_ref::<GenerateFailed>() {
            Some(failed) => watch::RunOutcome {
                ok: false,
                summary: format!("{} error(s), registry not written", failed.errors),
                detail: failed.rendered.concat(),
            },
            // Not an analysis failure: an unreadable source, an unparseable
            // config. Report it and keep watching — the fix is a save away.
            None => watch::RunOutcome {
                ok: false,
                summary: "generate failed".into(),
                detail: format!("{error}\n"),
            },
        },
    }
}

/// One watched `check` run, reduced to the log line `--watch` prints. Warnings
/// and hints are shown on a passing run too — that is what `check` reports, and
/// a watch that hid them would disagree with the one-shot command.
fn check_outcome(start_dir: &Path) -> watch::RunOutcome {
    let (ok, summary, rendered) = match run_check(start_dir) {
        Ok(passed) => (true, passed.summary, passed.rendered),
        Err(failed) => (false, failed.summary, failed.rendered),
    };
    watch::RunOutcome {
        ok,
        summary: format!(
            "{} source(s), {} diagnostic(s), {} error(s)",
            summary.sources_checked, summary.diagnostics, summary.errors
        ),
        detail: rendered.join("\n"),
    }
}

/// Where `generate` writes the registry. Shared with `--watch`, which must know
/// the path *before* the first run in order to exclude it from the watched
/// input set — a run that triggered itself would never stop.
fn generated_registry_path(root: &Path, out: Option<&Path>) -> PathBuf {
    out.map_or_else(|| root.join("surrealguard.generated.ts"), Path::to_path_buf)
}

/// Every host file (`.ts`/`.svelte`/…) under `root` that may carry embedded
/// SurrealQL, honoring the same ignore patterns as `.surql` discovery. Shared by
/// `check` and `generate` so the two commands can never disagree about which
/// files carry queries — a `check` that skipped them would pass a workspace
/// whose `generate` then fails.
fn discover_host_sources(root: &Path, config: &WorkspaceConfig) -> Vec<PathBuf> {
    let mut paths: Vec<_> = WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| should_visit(entry, root, &config.sources.ignore))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(DirEntry::into_path)
        .filter(|path| is_host_source(path))
        .collect();
    paths.sort();
    paths
}

/// Whether `path` is a host file that may carry embedded SurrealQL. Shared with
/// `--watch` so the watched set and the discovered set can never disagree —
/// a file the watcher ignores but `generate` reads would go silently stale,
/// which is the exact bug `--watch` exists to fix.
fn is_host_source(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("ts" | "tsx" | "js" | "jsx" | "svelte" | "vue" | "astro")
    )
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
        Commands::Generate { out, watch: true } => {
            let root = find_workspace_root(&env::current_dir()?);
            // Resolved, not merely computed: `--out` may be relative or may
            // point inside a watched directory, and an exclusion that doesn't
            // match the watcher's spelling of the path means `generate` sees
            // its own write and re-runs forever.
            let registry =
                watch::resolve_output(&root, &generated_registry_path(&root, out.as_deref()));
            let watch_root = root.clone();
            watch::watch_loop(&root, Some(&registry), move || {
                generate_outcome(&watch_root, out.as_deref())
            })
        }
        Commands::Generate { out, watch: false } => {
            let root = find_workspace_root(&env::current_dir()?);
            match run_generate(&root, out.as_deref()) {
                Ok(report) => {
                    // Warnings/hints don't block generation; surface them on stderr
                    // so the written registry stays the only thing on stdout.
                    for block in &report.warnings {
                        eprint!("{block}");
                    }
                    println!("Generated {}", report.path.display());
                    Ok(())
                }
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Check { watch: true, .. } => {
            let cwd = env::current_dir()?;
            let root = find_workspace_root(&cwd);
            // `check` writes nothing, so nothing needs excluding.
            watch::watch_loop(&root, None, move || check_outcome(&cwd))
        }
        Commands::Check { json, watch: false } => {
            if !json {
                println!("Checking SurrealQL sources...");
            }
            match run_check(&env::current_dir()?) {
                Ok(passed) => {
                    if json {
                        println!(
                            "{}",
                            render_check_json(&passed.summary, &passed.diagnostics)
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
            Commands::Check { json, watch } => {
                assert!(json);
                assert!(!watch);
            }
            _ => panic!("expected check command"),
        }
    }

    #[test]
    fn codegen_commands_are_not_part_of_the_rewrite_cli() {
        assert!(Cli::try_parse_from(["surrealguard", "run"]).is_err());
        // Still not a subcommand — and now deliberately so. Watching is a
        // *mode* of the two verbs that read the workspace, not a third verb
        // with its own semantics: `surrealguard watch` would have to answer
        // "watch and do what?" and would duplicate every flag of whichever
        // answer it picked. `check --watch` / `generate --watch` say it once.
        assert!(Cli::try_parse_from(["surrealguard", "watch"]).is_err());
    }

    #[test]
    fn both_workspace_reading_verbs_accept_watch() {
        let cli = Cli::try_parse_from(["surrealguard", "check", "--watch"]).expect("cli parses");
        match cli.command {
            Commands::Check { json, watch } => {
                assert!(watch);
                assert!(!json);
            }
            _ => panic!("expected check command"),
        }

        let cli = Cli::try_parse_from(["surrealguard", "generate", "--watch"]).expect("cli parses");
        match cli.command {
            Commands::Generate { out, watch } => {
                assert!(watch);
                assert!(out.is_none());
            }
            _ => panic!("expected generate command"),
        }
    }

    #[test]
    fn watch_composes_with_the_generate_output_path() {
        let cli = Cli::try_parse_from(["surrealguard", "generate", "--watch", "--out", "gen.ts"])
            .expect("cli parses");
        match cli.command {
            Commands::Generate { out, watch } => {
                assert!(watch);
                assert_eq!(out, Some(PathBuf::from("gen.ts")));
            }
            _ => panic!("expected generate command"),
        }
    }

    #[test]
    fn watch_and_json_are_mutually_exclusive() {
        // `--json` promises one document and one exit code for one run. A watch
        // stream is neither, so the pair is rejected at parse time rather than
        // silently emitting something no consumer can parse.
        assert!(Cli::try_parse_from(["surrealguard", "check", "--watch", "--json"]).is_err());
    }

    #[test]
    fn the_watched_registry_path_is_the_one_generate_writes() {
        // `--watch` must exclude the file `generate` writes, or every run
        // triggers the next one. The two must resolve the same path, including
        // the default.
        let root = temp_project_dir("registry-path");
        fs::write(root.join("surrealguard.toml"), "").expect("write config");
        fs::write(root.join("q.surql"), "DEFINE TABLE t;").expect("write source");

        let expected = generated_registry_path(&root, None);
        let report = run_generate(&root, None).expect("clean workspace generates");
        assert_eq!(report.path, expected);
        assert_eq!(expected, root.join("surrealguard.generated.ts"));

        let explicit = root.join("custom.ts");
        assert_eq!(
            generated_registry_path(&root, Some(&explicit)),
            explicit,
            "--out must be the excluded path when it is given"
        );
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

    #[test]
    fn generate_fails_on_embedded_query_error_and_names_host_file() {
        // A host file whose `db.query("...")` targets a missing table must fail
        // generation, name the host file, and leave no registry behind.
        let root = temp_project_dir("generate-bad-embed");
        fs::create_dir_all(root.join("schema")).expect("schema dir");
        fs::create_dir_all(root.join("src")).expect("src dir");
        fs::write(
            root.join("surrealguard.toml"),
            "[sources]\nschema = [\"schema/**/*.surql\"]\n",
        )
        .expect("write config");
        fs::write(
            root.join("schema/person.surql"),
            "DEFINE TABLE person SCHEMAFULL;\nDEFINE FIELD name ON person TYPE string;",
        )
        .expect("write schema");
        fs::write(
            root.join("src/app.ts"),
            "const [rows] = await db.query(\"SELECT nope FROM missing\");",
        )
        .expect("write host source");

        let out = root.join("surrealguard.generated.ts");
        let err = run_generate(&root, Some(&out))
            .expect_err("an error-severity embedded query must fail generate");
        let message = err.to_string();
        // The finding must map back to the host file at a real line:col (not the
        // degraded `file:start..end` offset form), with the query's table underlined.
        assert!(
            message.contains("app.ts:1:"),
            "findings must map to the host file at line:col: {message}"
        );
        assert!(
            message.contains("db.query(\"SELECT nope FROM missing\")"),
            "the rendered snippet must show the host source line: {message}"
        );
        assert!(
            message.contains("^"),
            "the rendered snippet must underline the offending span: {message}"
        );
        assert!(
            message.contains("registry not written"),
            "failure must explain the registry was withheld: {message}"
        );
        assert!(
            !out.exists(),
            "a broken registry must never be written on an error finding"
        );
    }

    #[test]
    fn generate_writes_registry_for_clean_embedded_queries() {
        // A clean `db.query("...")` resolves its result and params from the
        // schema and lands in the registry, keyed by the exact query text.
        let root = temp_project_dir("generate-clean-embed");
        fs::create_dir_all(root.join("schema")).expect("schema dir");
        fs::create_dir_all(root.join("src")).expect("src dir");
        fs::write(
            root.join("surrealguard.toml"),
            "[sources]\nschema = [\"schema/**/*.surql\"]\n",
        )
        .expect("write config");
        fs::write(
            root.join("schema/person.surql"),
            "DEFINE TABLE person SCHEMAFULL;\n\
             DEFINE FIELD name ON person TYPE string;\n\
             DEFINE FIELD team ON person TYPE record<team>;\n\
             DEFINE TABLE team SCHEMAFULL;\n\
             DEFINE FIELD name ON team TYPE string;",
        )
        .expect("write schema");
        fs::write(
            root.join("src/app.ts"),
            "const [rows] = await db.query(\"SELECT name FROM person WHERE team = $team\", { team });",
        )
        .expect("write host source");

        let out = root.join("surrealguard.generated.ts");
        let report =
            run_generate(&root, Some(&out)).expect("a clean embedded query should generate");
        assert_eq!(report.path, out);
        let written = fs::read_to_string(&out).expect("registry file written");
        assert!(
            written.contains("SELECT name FROM person WHERE team = $team"),
            "registry must key the embedded query by its exact text:\n{written}"
        );
        assert!(
            written.contains("params: { team: RecordId<\"team\"> }"),
            "the $team param must be typed from the schema record link:\n{written}"
        );
    }

    #[test]
    fn check_reports_errors_in_embedded_host_queries_at_the_host_file() {
        // `check` is the CI gate. It must see the queries the client actually
        // runs — a host file's `surql` template — or CI passes green on a
        // workspace whose `generate` then fails.
        let root = temp_project_dir("check-embedded-error");
        fs::create_dir_all(root.join("schema")).expect("schema dir");
        fs::create_dir_all(root.join("src")).expect("src dir");
        fs::write(
            root.join("surrealguard.toml"),
            "[sources]\nschema = [\"schema/**/*.surql\"]\n",
        )
        .expect("write config");
        fs::write(root.join("schema/t.surql"), "DEFINE TABLE t SCHEMAFULL;").expect("write schema");
        fs::write(
            root.join("src/app.ts"),
            "const q = surql`SELECT * FROM nonexistent_table;`;",
        )
        .expect("write host source");

        let err = run_check(&root).expect_err("an embedded unknown table must fail the check");
        assert!(
            err.diagnostics.iter().any(|d| d.code.starts_with("E1001")),
            "expected unknown-table E1001, got {:?}",
            err.diagnostics.iter().map(|d| &d.code).collect::<Vec<_>>()
        );
        // The finding must be reported against the host file, not the internal
        // `embedded://…` source the query was analyzed under.
        assert!(
            err.diagnostics
                .iter()
                .any(|d| d.source.contains("app.ts") && !d.source.starts_with("embedded://")),
            "embedded findings must map to the host file: {:?}",
            err.diagnostics
                .iter()
                .map(|d| &d.source)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn check_passes_a_valid_embedded_host_query() {
        // The mirror of the above: scanning host files must not invent findings
        // on a query that is perfectly valid against the schema.
        let root = temp_project_dir("check-embedded-clean");
        fs::create_dir_all(root.join("schema")).expect("schema dir");
        fs::create_dir_all(root.join("src")).expect("src dir");
        fs::write(
            root.join("surrealguard.toml"),
            "[sources]\nschema = [\"schema/**/*.surql\"]\n",
        )
        .expect("write config");
        fs::write(
            root.join("schema/t.surql"),
            "DEFINE TABLE t SCHEMAFULL;\nDEFINE FIELD price ON t TYPE int;",
        )
        .expect("write schema");
        fs::write(
            root.join("src/app.ts"),
            "const q = surql`SELECT price FROM t;`;",
        )
        .expect("write host source");

        let passed = run_check(&root).expect("a valid embedded query must pass");
        assert_eq!(passed.summary.errors, 0);
    }

    #[test]
    fn check_json_lists_surviving_warnings_on_a_clean_run() {
        // A clean run is not a silent run. The summary counts warnings, so the
        // `diagnostics` array must carry them too — otherwise `--json`
        // contradicts itself and tooling sees an empty list.
        let root = temp_project_dir("check-json-clean-warnings");
        fs::write(root.join("surrealguard.toml"), "").expect("write config");
        fs::write(
            root.join("schema.surql"),
            "DEFINE TABLE t DROP SCHEMAFULL;\nDEFINE FIELD x ON t TYPE int;\nSELECT * FROM t;",
        )
        .expect("write source");

        let passed = run_check(&root).expect("warning-only source should pass");
        assert_eq!(passed.summary.errors, 0);
        assert_eq!(passed.summary.diagnostics, 1);
        let json = render_check_json(&passed.summary, &passed.diagnostics).expect("json renders");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(
            value["diagnostics"].as_array().map(Vec::len),
            Some(1),
            "a passing run must still list its warnings: {json}"
        );
        assert_eq!(value["diagnostics"][0]["severity"], "warning");
    }

    pub(crate) fn temp_project_dir(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time is after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("surrealguard-{name}-{unique}"));
        fs::create_dir_all(&root).expect("create temp project root");
        root
    }
}
