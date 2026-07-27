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
//! - `surrealguard watch [--out PATH] [--check-only]` — the development loop:
//!   check, then regenerate, on every change. See [`watch`].
//!
//! `--watch` turns either verb into a loop too: run once, then re-run on every
//! change to an input the analysis consumes (`.surql` sources, host files
//! carrying embedded queries, and `surrealguard.toml`).
//!
//! # Output
//!
//! Human output is coloured when — and only when — it is going to a terminal
//! that wants colour: `--no-color`, `NO_COLOR`, `TERM=dumb` and a redirected
//! stream each turn it off, so `surrealguard check > report.txt` is clean text.
//! See [`style`]. `--json` is a machine contract (one document, one exit code)
//! and is never decorated.
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
mod spinner;
mod style;
mod watch;

use clap::{Parser, Subcommand};
use serde::Serialize;
use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Instant;
use style::{count, tint, Outcome, Styles};
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

    /// Never emit ANSI colour, even on a terminal.
    ///
    /// Colour is already off when the output is not a terminal, when `NO_COLOR`
    /// is set, and when `TERM=dumb`; this is the explicit override for a
    /// terminal that reports colour support it does not have.
    #[arg(long, global = true)]
    no_color: bool,
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

    /// Check and regenerate on every change — the loop to run beside a dev server
    ///
    /// Equivalent to `check --watch` and `generate --watch` in one process,
    /// which is what a development session actually needs: `check` alone leaves
    /// the generated types stale, and `generate` alone reports only the
    /// findings of the queries it embeds, so a broken `.surql` file passes
    /// unmentioned. A run that fails the check does not write the registry.
    Watch {
        /// Output path for the generated module (default:
        /// surrealguard.generated.ts at the workspace root).
        #[arg(long)]
        out: Option<std::path::PathBuf>,

        /// Only check. Nothing is written, so this is the safe mode for a
        /// workspace whose registry is committed and reviewed.
        #[arg(long)]
        check_only: bool,
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

/// Analyzes the workspace containing `start_dir` and renders every surviving
/// finding.
///
/// `styles` decorates the rendered blocks. Both streams are handed in because
/// the destination — stderr on a failing run, stdout on a passing one — is what
/// decides whether an escape sequence is safe to write, and this function is
/// the first place that knows which one it will be.
/// The embedded queries a `check` analyzes, keyed by the virtual source id they
/// were added under, with the host file each came from.
type EmbeddedQueries =
    std::collections::BTreeMap<String, (surrealguard_embed::EmbeddedQuery, String)>;

/// Adds every `surql` template found in a host file to `workspace` as a virtual
/// source, and records the host file's text so findings can be rendered against
/// real source.
///
/// Embedded queries in host files (`surql` tagged templates in `.ts`/`.svelte`/…)
/// are part of the workspace: they are the queries the client actually runs.
/// `generate` has always analyzed them, so a `check` that ignored them would
/// pass a workspace whose `generate` then fails with errors — CI green, build
/// broken. Their findings are remapped to `host_file:line` by the caller.
fn add_embedded_queries(
    workspace: &mut Workspace,
    root: &Path,
    config: &WorkspaceConfig,
    source_texts: &mut std::collections::BTreeMap<String, String>,
) -> EmbeddedQueries {
    let mut embedded = EmbeddedQueries::new();
    for path in discover_host_sources(root, config) {
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
    embedded
}

fn run_check(start_dir: &Path, styles: StylePair) -> Result<CheckPassed, CheckFailed> {
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

    let embedded = add_embedded_queries(&mut workspace, &root, &config, &mut source_texts);

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
    let errors = resolved
        .iter()
        .filter(|(_, severity)| *severity == Severity::Error)
        .count();
    let block_styles = styles.for_check(errors > 0);
    let rendered: Vec<String> = resolved
        .iter()
        .map(|(finding, severity)| {
            render::render_finding(finding, *severity, &source_texts, block_styles)
        })
        .collect();
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
    /// The rendered "`@surrealguard/client` is not installed" block, when the
    /// package the written module augments cannot be resolved from its
    /// directory. See [`client_package_is_resolvable`] for why this is fatal in
    /// practice and silent without it.
    missing_client: Option<String>,
}

/// The npm package the generated module imports from and augments.
const CLIENT_PACKAGE: &str = "@surrealguard/client";

/// Whether Node/TypeScript would resolve [`CLIENT_PACKAGE`] from `dir`, by the
/// rule they both use: walk up from the importing file's directory and take the
/// first `node_modules` that contains the package.
///
/// This is the difference between a generated file that types everything and
/// one that types nothing. The module ends in
/// `declare module "@surrealguard/client" { … }`, and a module augmentation is
/// only an augmentation if the target resolves. When it does not, TypeScript
/// reports `TS2664: Invalid module name in augmentation` **inside the generated
/// file** and drops the block — so the user's own `db.query(…)` keeps
/// compiling, silently as `any`, with no error anywhere near it. Nothing about
/// that failure points at the missing dependency, which is why `generate` has
/// to say it out loud.
///
/// `package.json` is the marker rather than the directory, because a leftover
/// empty `node_modules/@surrealguard/client/` resolves for neither tool.
fn client_package_is_resolvable(dir: &Path) -> bool {
    let scope_path: PathBuf = CLIENT_PACKAGE.split('/').collect();
    let mut current = Some(dir);
    while let Some(directory) = current {
        if directory
            .join("node_modules")
            .join(&scope_path)
            .join("package.json")
            .exists()
        {
            return true;
        }
        current = directory.parent();
    }
    false
}

/// The warning printed when the written module augments a package that is not
/// installed. Shaped like a finding — header, location, explanation, fix — so
/// it reads in the same language as everything else `generate` prints.
fn missing_client_warning(out_path: &Path, styles: Styles) -> String {
    let bar = styles.frame("  |");
    let mut out = format!(
        "{}: {}\n",
        styles.severity(Severity::Warning, "warning"),
        styles.message(&format!("`{CLIENT_PACKAGE}` is not installed"))
    );
    out.push_str(&format!(
        "  {} {}\n",
        styles.frame("-->"),
        styles.path(&display_path(out_path))
    ));
    out.push_str(&format!("{bar}\n"));
    out.push_str(&format!(
        "{bar} this file augments `declare module \"{CLIENT_PACKAGE}\"`, and an\n"
    ));
    out.push_str(&format!(
        "{bar} augmentation whose target does not resolve is silently dropped —\n"
    ));
    out.push_str(&format!(
        "{bar} TypeScript reports TS2664 here, and every query typed through it\n"
    ));
    out.push_str(&format!("{bar} degrades to `any` with no error on it.\n"));
    out.push_str(&format!("{bar}\n"));
    out.push_str(&format!(
        "  {} {} npm install {CLIENT_PACKAGE} surrealdb\n",
        styles.frame("="),
        styles.label("help:"),
    ));
    out
}

/// A path relative to the working directory when it is under it, so output
/// reads `src/surrealguard.generated.ts` rather than an absolute path.
fn display_path(path: &Path) -> String {
    env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok())
        .unwrap_or(path)
        .display()
        .to_string()
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
fn run_generate(
    root: &Path,
    out: Option<&Path>,
    styles: Styles,
) -> Result<GenerateReport, Box<dyn Error>> {
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
            let block = render::render_finding(&host_finding, severity, &host_texts, styles);
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
                    // A tuple slot: no key to omit, so an `option<T>` result
                    // stays `T | undefined` rather than becoming optional.
                    statement.response_kind.as_ref().map_or_else(
                        || "null".into(),
                        |kind| {
                            surrealguard_codegen::ts_type(
                                kind,
                                surrealguard_codegen::TsContext::Value,
                            )
                            .text
                        },
                    )
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
    let module = surrealguard_codegen::render_registry(&entries);
    fs::write(&out_path, &module)?;

    // Resolution is asked from the *written module's* directory, not the
    // workspace root: that is the directory TypeScript resolves the import
    // from, and in a monorepo the two are routinely different packages.
    let missing_client = (module.contains(CLIENT_PACKAGE)
        && !client_package_is_resolvable(out_path.parent().unwrap_or(root)))
    .then(|| missing_client_warning(&out_path, styles));

    Ok(GenerateReport {
        path: out_path,
        warnings: rendered_warnings,
        queries: entries.len(),
        missing_client,
    })
}

/// One watched `generate` run, reduced to the log line `--watch` prints.
///
/// A failure is reported and returned, never propagated: a transient error —
/// the syntax error you are halfway through typing — must not end the watch.
fn generate_outcome(root: &Path, out: Option<&Path>, styles: Styles) -> watch::RunOutcome {
    match run_generate(root, out, styles) {
        Ok(report) => {
            let mut summary = format!(
                "wrote {} · {}",
                display_relative(root, &report.path),
                count(report.queries, "query")
            );
            let mut detail = report.warnings.join("\n");
            let mut outcome = Outcome::of(report.warnings.len(), 0);
            if let Some(warning) = report.missing_client {
                summary.push_str(&format!(" · {CLIENT_PACKAGE} not installed"));
                detail.push_str(&warning);
                outcome = Outcome::Warned;
            }
            watch::RunOutcome {
                outcome,
                summary,
                detail,
            }
        }
        // The findings already say everything the trailing "generate failed:"
        // line would, and the run line above carries the count — so print the
        // blocks only, and keep the line short enough to scan when it repeats.
        Err(error) => match error.downcast_ref::<GenerateFailed>() {
            Some(failed) => watch::RunOutcome {
                outcome: Outcome::Failed,
                summary: format!("{} · registry not written", count(failed.errors, "error")),
                detail: failed.rendered.join("\n"),
            },
            // Not an analysis failure: an unreadable source, an unparseable
            // config. Report it and keep watching — the fix is a save away.
            None => watch::RunOutcome {
                outcome: Outcome::Failed,
                summary: "generate failed".into(),
                detail: format!("{error}\n"),
            },
        },
    }
}

/// One watched run of the bare `watch` verb: check the whole workspace, then —
/// only if it passed — regenerate.
///
/// The ordering is the justification for the pairing. `check` is the surface
/// that sees every input, including `.surql` query files that never reach the
/// registry; `generate` is what keeps the TypeScript honest. Running generate
/// second and only on a clean check means a broken save never overwrites a
/// working registry, and the editor keeps the last types that compiled.
///
/// `generate`'s own findings are not printed here: they are the embedded-query
/// subset of the findings `check` just printed, so re-emitting them would
/// double every diagnostic in a host file.
fn watch_outcome(
    root: &Path,
    start_dir: &Path,
    out: Option<&Path>,
    check_only: bool,
    styles: Styles,
) -> watch::RunOutcome {
    let mut checked = check_outcome(start_dir, styles);
    if check_only || checked.outcome == Outcome::Failed {
        if !check_only {
            checked.summary.push_str(" · registry not written");
        }
        return checked;
    }
    match run_generate(root, out, styles) {
        Ok(report) => {
            checked.summary = format!(
                "{} · wrote {} ({})",
                checked.summary,
                display_relative(root, &report.path),
                count(report.queries, "query")
            );
            if let Some(warning) = report.missing_client {
                checked
                    .summary
                    .push_str(&format!(" · {CLIENT_PACKAGE} not installed"));
                checked.detail.push_str(&warning);
                checked.outcome = Outcome::Warned;
            }
            checked
        }
        Err(error) => watch::RunOutcome {
            outcome: Outcome::Failed,
            summary: format!("{} · generate failed", checked.summary),
            detail: format!("{}\n{error}\n", checked.detail),
        },
    }
}

/// One watched `check` run, reduced to the log line `--watch` prints. Warnings
/// and hints are shown on a passing run too — that is what `check` reports, and
/// a watch that hid them would disagree with the one-shot command.
fn check_outcome(start_dir: &Path, styles: Styles) -> watch::RunOutcome {
    // The watch loop prints every stream to stdout, so both halves of the pair
    // are the same styling — a split would reorder the blocks the moment the
    // log is piped.
    let (summary, rendered) = match run_check(start_dir, StylePair::both(styles)) {
        Ok(passed) => (passed.summary, passed.rendered),
        Err(failed) => (failed.summary, failed.rendered),
    };
    watch::RunOutcome {
        outcome: Outcome::of(summary.diagnostics, summary.errors),
        summary: format!(
            "{} · {}",
            count(summary.sources_checked, "source"),
            finding_tally(&summary)
        ),
        // Joined, not concatenated: each rendered block ends in a newline, so
        // concatenation would run one diagnostic straight into the next.
        detail: rendered.join("\n"),
    }
}

/// `no diagnostics` / `2 warnings` / `3 errors, 1 warning` — the phrase that
/// has to make "clean" and "broken" different at a glance.
///
/// Subtracting is exact rather than approximate: policy resolution collapses
/// every surviving finding to `Warning` or `Error` (a `Hint`-class code is
/// graded by its lint level like everything else), so the non-errors are all
/// warnings.
fn finding_tally(summary: &CheckSummary) -> String {
    let warnings = summary.diagnostics.saturating_sub(summary.errors);
    match (summary.errors, warnings) {
        (0, 0) => "no diagnostics".into(),
        (0, w) => count(w, "warning"),
        (e, 0) => count(e, "error"),
        (e, w) => format!("{}, {}", count(e, "error"), count(w, "warning")),
    }
}

/// A path shown relative to the workspace root when it lives under it.
fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
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

/// The styling for each of the two output streams.
///
/// Two, not one, because the destination is what decides whether an escape
/// sequence is safe: `surrealguard check > report.txt` must write clean text to
/// the file while the errors that stay on the terminal keep their colour, and
/// `2>/dev/null` is the same argument in reverse.
#[derive(Clone, Copy)]
struct StylePair {
    stdout: Styles,
    stderr: Styles,
}

impl StylePair {
    fn resolve(no_color: bool) -> Self {
        Self {
            stdout: Styles::for_stream(std::io::stdout().is_terminal(), no_color),
            stderr: Styles::for_stream(std::io::stderr().is_terminal(), no_color),
        }
    }

    /// Both streams styled the same. The watch loop prints everything to
    /// stdout, and `--json` and the tests want no styling at all.
    const fn both(styles: Styles) -> Self {
        Self {
            stdout: styles,
            stderr: styles,
        }
    }

    /// The styling for the stream a check's blocks will land on: stderr when
    /// the run failed, stdout when it passed.
    const fn for_check(self, failed: bool) -> Styles {
        if failed {
            self.stderr
        } else {
            self.stdout
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let styles = StylePair::resolve(cli.no_color);

    match cli.command {
        Commands::Init => command_init(styles.stdout),
        Commands::Generate { out, watch } => command_generate(out, watch, styles),
        Commands::Check { json, watch } => command_check(json, watch, styles),
        Commands::Watch { out, check_only } => command_watch(out, check_only, styles.stdout),
    }
}

fn command_init(styles: Styles) -> Result<(), Box<dyn Error>> {
    let config_path = env::current_dir()?.join("surrealguard.toml");
    if config_path.exists() {
        println!(
            "{} surrealguard.toml already exists at {}",
            styles.warn("skipped"),
            styles.path(&display_path(&config_path))
        );
        return Ok(());
    }

    fs::write(&config_path, EXAMPLE_CONFIG)?;
    println!(
        "{} {}",
        styles.ok("created"),
        styles.path("surrealguard.toml")
    );
    println!(
        "{}",
        styles.dim("next: point [sources] at your .surql files, then run `surrealguard check`")
    );
    Ok(())
}

fn command_generate(
    out: Option<PathBuf>,
    watch_mode: bool,
    styles: StylePair,
) -> Result<(), Box<dyn Error>> {
    let root = find_workspace_root(&env::current_dir()?);
    if watch_mode {
        // Resolved, not merely computed: `--out` may be relative or may
        // point inside a watched directory, and an exclusion that doesn't
        // match the watcher's spelling of the path means `generate` sees
        // its own write and re-runs forever.
        let registry =
            watch::resolve_output(&root, &generated_registry_path(&root, out.as_deref()));
        let watch_root = root.clone();
        return watch::watch_loop(&root, Some(&registry), styles.stdout, move || {
            generate_outcome(&watch_root, out.as_deref(), styles.stdout)
        });
    }

    let started = Instant::now();
    let spinner = spinner::Spinner::start("Generating", styles.stdout.is_colored());
    let result = run_generate(&root, out.as_deref(), styles.stderr);
    spinner.finish();

    match result {
        Ok(report) => {
            // Warnings/hints don't block generation; surface them on stderr
            // so the written registry stays the only thing on stdout.
            for block in &report.warnings {
                eprint!("{block}");
            }
            if let Some(warning) = &report.missing_client {
                eprint!("{warning}");
            }
            let styles = styles.stdout;
            println!(
                "{} {} {}",
                styles.ok("generated"),
                styles.path(&display_relative(&root, &report.path)),
                styles.dim(&format!(
                    "({}, {})",
                    count(report.queries, "query"),
                    elapsed(started)
                ))
            );
            Ok(())
        }
        Err(error) => {
            let styles = styles.stderr;
            match error.downcast_ref::<GenerateFailed>() {
                Some(failed) => {
                    for block in &failed.rendered {
                        eprintln!("{block}");
                    }
                    eprintln!(
                        "{} {}",
                        styles.fail(&count(failed.errors, "error")),
                        styles.dim("in embedded queries — registry not written")
                    );
                }
                None => eprintln!("{} {error}", styles.fail("error:")),
            }
            std::process::exit(1);
        }
    }
}

fn command_check(json: bool, watch_mode: bool, styles: StylePair) -> Result<(), Box<dyn Error>> {
    let cwd = env::current_dir()?;
    if watch_mode {
        let root = find_workspace_root(&cwd);
        // `check` writes nothing, so nothing needs excluding.
        return watch::watch_loop(&root, None, styles.stdout, move || {
            check_outcome(&cwd, styles.stdout)
        });
    }

    // `--json` is a machine contract: one document, one exit code, and no
    // decoration anywhere near it — not a spinner, not an escape sequence.
    let render_styles = if json {
        StylePair::both(Styles::plain())
    } else {
        styles
    };

    let started = Instant::now();
    let spinner = spinner::Spinner::start(
        "Checking SurrealQL sources",
        !json && styles.stdout.is_colored(),
    );
    let result = run_check(&cwd, render_styles);
    spinner.finish();

    match result {
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
                println!("{}", check_summary(&passed.summary, started, styles.stdout));
            }
            Ok(())
        }
        Err(failed) => {
            if json {
                println!(
                    "{}",
                    render_check_json(&failed.summary, &failed.diagnostics)
                        .expect("json serialization should not fail")
                );
            } else {
                for block in &failed.rendered {
                    eprintln!("{block}");
                }
                eprintln!("{}", check_summary(&failed.summary, started, styles.stderr));
            }
            std::process::exit(1);
        }
    }
}

/// The two closing lines of a `check`: what was read, and what was found.
///
/// Split in two on purpose. The first is bookkeeping and stays dim; the second
/// is the answer, and it carries the run's colour — green when there is nothing
/// to fix, red when there is. That is the line someone reads from three feet
/// away, so it must not look the same in both cases.
fn check_summary(summary: &CheckSummary, started: Instant, styles: Styles) -> String {
    let bookkeeping = styles.dim(&format!(
        "checked {} in {}",
        count(summary.sources_checked, "source"),
        elapsed(started)
    ));
    let outcome = Outcome::of(summary.diagnostics, summary.errors);
    let verdict = match outcome {
        Outcome::Clean => "no issues found".to_string(),
        _ => format!("found {}", finding_tally(summary)),
    };
    format!("{bookkeeping}\n{}", tint(styles, outcome, &verdict))
}

fn command_watch(
    out: Option<PathBuf>,
    check_only: bool,
    styles: Styles,
) -> Result<(), Box<dyn Error>> {
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&cwd);
    // Nothing is written in `--check-only`, so nothing needs excluding; the
    // registry does, and for the same reason `generate --watch` excludes it.
    let registry = (!check_only)
        .then(|| watch::resolve_output(&root, &generated_registry_path(&root, out.as_deref())));
    let watch_root = root.clone();
    watch::watch_loop(&root, registry.as_deref(), styles, move || {
        watch_outcome(&watch_root, &cwd, out.as_deref(), check_only, styles)
    })
}

/// A duration a human reads: milliseconds until they stop being small.
fn elapsed(started: Instant) -> String {
    let millis = started.elapsed().as_millis();
    if millis < 1000 {
        format!("{millis}ms")
    } else {
        format!("{:.2}s", started.elapsed().as_secs_f64())
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

        let failed = run_check(&root, StylePair::both(Styles::plain()))
            .expect_err("the unknown field should fail the check");
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

        let summary = run_check(&root, StylePair::both(Styles::plain()))
            .expect("valid workspace should check");

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

        let err = run_check(&root, StylePair::both(Styles::plain()))
            .expect_err("syntax diagnostics should fail check");

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

        let summary = run_check(&child, StylePair::both(Styles::plain()))
            .expect("valid parent workspace should check");

        assert_eq!(summary.summary.sources_checked, 1);
    }

    #[test]
    fn check_json_output_uses_stable_diagnostic_keys() {
        let root = temp_project_dir("json-check");
        fs::create_dir_all(root.join("queries")).expect("create queries dir");
        fs::write(root.join("surrealguard.toml"), "").expect("write config");
        fs::write(root.join("queries/bad.surql"), "SELECT * FROM ;").expect("write query");

        let err = run_check(&root, StylePair::both(Styles::plain()))
            .expect_err("syntax diagnostics should fail check");
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
    }

    #[test]
    fn bare_watch_is_a_verb_and_it_means_check_then_generate() {
        // It used to not exist, on the argument that watching is a *mode* of
        // the two verbs rather than a verb of its own. That argument was about
        // the implementation; the cost landed on users, who did not find
        // `--watch` at all. The question it was said to be unable to answer —
        // "watch and do what?" — has an answer, and it is the one a dev server
        // needs: check everything, then regenerate what compiles.
        let cli = Cli::try_parse_from(["surrealguard", "watch"]).expect("watch is a verb");
        match cli.command {
            Commands::Watch { out, check_only } => {
                assert!(out.is_none());
                assert!(!check_only, "bare watch generates as well as checks");
            }
            _ => panic!("expected watch command"),
        }
    }

    #[test]
    fn watch_takes_the_generate_output_path_and_a_check_only_mode() {
        let cli = Cli::try_parse_from(["surrealguard", "watch", "--out", "gen.ts"])
            .expect("watch --out parses");
        match cli.command {
            Commands::Watch { out, .. } => assert_eq!(out, Some(PathBuf::from("gen.ts"))),
            _ => panic!("expected watch command"),
        }

        let cli = Cli::try_parse_from(["surrealguard", "watch", "--check-only"])
            .expect("watch --check-only parses");
        match cli.command {
            Commands::Watch { check_only, .. } => assert!(check_only),
            _ => panic!("expected watch command"),
        }
    }

    #[test]
    fn the_existing_watch_flags_keep_working() {
        // Adding the verb must not retire the flags: they are in scripts,
        // CI configs and READMEs already.
        for args in [
            vec!["surrealguard", "check", "--watch"],
            vec!["surrealguard", "generate", "--watch"],
            vec!["surrealguard", "generate", "--watch", "--out", "gen.ts"],
        ] {
            assert!(
                Cli::try_parse_from(&args).is_ok(),
                "{args:?} must still parse"
            );
        }
    }

    #[test]
    fn no_color_is_accepted_on_every_verb() {
        // A global flag, because the user who wants plain output wants it from
        // whichever command they happened to type.
        for verb in ["check", "generate", "watch"] {
            let cli = Cli::try_parse_from(["surrealguard", verb, "--no-color"])
                .unwrap_or_else(|error| panic!("{verb} --no-color: {error}"));
            assert!(cli.no_color);
        }
    }

    #[test]
    fn a_watched_run_separates_its_diagnostic_blocks() {
        // Each rendered block ends in a newline; concatenating them runs one
        // diagnostic's caret line straight into the next one's header.
        let root = temp_project_dir("watch-detail-spacing");
        fs::write(root.join("surrealguard.toml"), "").expect("write config");
        fs::write(
            root.join("q.surql"),
            "SELECT * FROM ghost;\nSELECT * FROM phantom;",
        )
        .expect("write source");

        let outcome = check_outcome(&root, Styles::plain());
        assert_eq!(outcome.outcome, Outcome::Failed);
        assert!(
            outcome.detail.contains("\n\nerror["),
            "blocks must be separated by a blank line:\n{}",
            outcome.detail
        );
    }

    #[test]
    fn the_tally_distinguishes_clean_from_warned_from_failed() {
        // The one phrase that has to be unmistakable at a glance.
        let tally = |diagnostics, errors| {
            finding_tally(&CheckSummary {
                sources_checked: 1,
                diagnostics,
                errors,
            })
        };
        assert_eq!(tally(0, 0), "no diagnostics");
        assert_eq!(tally(2, 0), "2 warnings");
        assert_eq!(tally(1, 1), "1 error");
        assert_eq!(tally(3, 2), "2 errors, 1 warning");
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
        let report = run_generate(&root, None, Styles::plain()).expect("clean workspace generates");
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

        let err = run_check(&root, StylePair::both(Styles::plain()))
            .expect_err("promoted warning should fail the check");

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
        let summary = run_check(&root, StylePair::both(Styles::plain()))
            .expect("warning-only source should pass");
        assert_eq!(summary.summary.diagnostics, 1);
        assert_eq!(summary.summary.errors, 0);
    }

    #[test]
    fn error_finding_fails_the_check_without_any_policy() {
        let root = temp_project_dir("error-fails");
        fs::write(root.join("surrealguard.toml"), "").expect("write config");
        // An unknown table is an error-class finding (E1001).
        fs::write(root.join("query.surql"), "SELECT * FROM ghost;").expect("write source");

        let err = run_check(&root, StylePair::both(Styles::plain()))
            .expect_err("error finding should fail the check");

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
        let err = run_generate(&root, Some(&out), Styles::plain())
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
        let report = run_generate(&root, Some(&out), Styles::plain())
            .expect("a clean embedded query should generate");
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

        let err = run_check(&root, StylePair::both(Styles::plain()))
            .expect_err("an embedded unknown table must fail the check");
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

        let passed = run_check(&root, StylePair::both(Styles::plain()))
            .expect("a valid embedded query must pass");
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

        let passed = run_check(&root, StylePair::both(Styles::plain()))
            .expect("warning-only source should pass");
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

    /// Writes a resolvable `@surrealguard/client` under `dir/node_modules`.
    fn install_client(dir: &Path) {
        let package = dir.join("node_modules/@surrealguard/client");
        fs::create_dir_all(&package).expect("create package dir");
        fs::write(
            package.join("package.json"),
            "{\"name\":\"@surrealguard/client\"}",
        )
        .expect("write package.json");
    }

    /// The workspace `generate` needs to produce a module at all: a schema and
    /// one clean embedded query.
    fn generatable_project(name: &str) -> std::path::PathBuf {
        let root = temp_project_dir(name);
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
            "const [rows] = await db.query(\"SELECT name FROM person\");",
        )
        .expect("write host source");
        root
    }

    #[test]
    fn generate_warns_when_the_augmented_package_is_not_installed() {
        // The worst failure a type generator has: the module augments
        // `@surrealguard/client`, the package is absent, TypeScript reports
        // TS2664 *inside the generated file*, drops the augmentation, and every
        // query in the user's own code silently becomes `any` with no error on
        // it. Nothing in that chain points at the missing dependency, so
        // `generate` has to.
        let root = generatable_project("generate-missing-client");

        let report = run_generate(&root, None, Styles::plain()).expect("generate writes");
        let warning = report
            .missing_client
            .expect("an unresolvable augmentation target must be reported");

        assert!(warning.contains("`@surrealguard/client` is not installed"));
        assert!(
            warning.contains("npm install @surrealguard/client surrealdb"),
            "the warning must name the command that fixes it: {warning}"
        );
        assert!(
            warning.contains("TS2664"),
            "naming the error TypeScript reports is what makes it searchable: {warning}"
        );
        assert!(
            warning.contains("`any`"),
            "the consequence is the reason this warning exists: {warning}"
        );
    }

    #[test]
    fn generate_is_silent_when_the_package_resolves() {
        let root = generatable_project("generate-client-present");
        install_client(&root);

        let report = run_generate(&root, None, Styles::plain()).expect("generate writes");
        assert!(
            report.missing_client.is_none(),
            "an installed package must not warn: {:?}",
            report.missing_client
        );
    }

    #[test]
    fn resolution_walks_up_from_the_output_directory_the_way_node_does() {
        // The module is resolved from the directory it is written to, not from
        // the workspace root — a hoisted install two directories up resolves,
        // and in a monorepo those are routinely different packages.
        let root = generatable_project("generate-client-hoisted");
        let nested = root.join("packages/app/src");
        fs::create_dir_all(&nested).expect("nested dirs");
        install_client(&root);

        assert!(client_package_is_resolvable(&nested), "hoisted install");
        assert!(
            !client_package_is_resolvable(Path::new("/")),
            "a tree with no node_modules must not resolve"
        );

        // An empty package directory is not an install: neither Node nor
        // TypeScript resolves one, so neither does this.
        let bare = temp_project_dir("generate-client-empty-dir");
        fs::create_dir_all(bare.join("node_modules/@surrealguard/client")).expect("empty package");
        assert!(!client_package_is_resolvable(&bare));
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
