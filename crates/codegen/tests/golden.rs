//! Golden test for the generated TypeScript — the harness that catches a
//! generated file that does not compile.
//!
//! `surrealguard generate` writes a module (`SurrealGuardClient`, the query
//! registry, the response types) and until this test nothing ever handed that
//! module to `tsc`: a type error in the emitter's output would ship, and the
//! user would be the first to see it. The check has two halves that meet at
//! one committed file:
//!
//! 1. **This test** runs the CLI's generation path — the same
//!    `QueryEntry::from_analysis` + `render_registry` `run_generate` calls —
//!    over the fixture workspace at `tests/fixtures/typecheck/` and compares
//!    the result byte for byte with the committed golden,
//!    `packages/client/test-d/gen/surrealguard.generated.ts`.
//! 2. **`pnpm -r run typecheck`** compiles that golden as part of
//!    `@surrealguard/client`, against the package's own source and the real
//!    `surrealdb` types, and `test-d/gen/*.test-d.ts` plus
//!    `test/generated.test.ts` assert what the resolved types are.
//!
//! So a generator change that alters the output fails here until the golden
//! is regenerated, and a regenerated golden that no longer typechecks fails
//! in CI's package job. Regenerate with:
//!
//! ```text
//! UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-codegen --test golden
//! ```
//!
//! The golden records *current* output, not correct output: read the diff
//! before accepting it, and run the package typecheck after.

use std::path::{Path, PathBuf};

use surrealguard_codegen::{render_registry, QueryEntry};
use surrealguard_diagnostics::Severity;
use surrealguard_workspace::config::WorkspaceConfig;
use surrealguard_workspace::{analyze_workspace, Workspace};

/// The fixture workspace: a `surrealguard.toml`, schema and query `.surql`
/// files, and a host `src/queries.ts` carrying the embedded queries.
fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/typecheck")
}

/// The committed golden. It lives in the client package, not beside this
/// test, because the package's `tsconfig` is what compiles it.
fn golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/client/test-d/gen/surrealguard.generated.ts")
}

fn updating() -> bool {
    std::env::var_os("UPDATE_SNAPSHOTS").is_some()
}

#[test]
fn generated_module_matches_the_committed_golden() {
    let actual = generate(&fixture_root());
    let path = golden_path();

    if updating() {
        std::fs::create_dir_all(path.parent().expect("golden dir")).expect("create golden dir");
        std::fs::write(&path, &actual).expect("write golden");
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing golden {}\n\
             create it with: UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-codegen --test golden",
            path.display()
        )
    });

    if expected != actual {
        // Leave the full actual output where a `diff` can reach it: the
        // first-difference excerpt below is for orientation, not review.
        let actual_path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("surrealguard.generated.ts");
        std::fs::write(&actual_path, &actual).expect("write actual output");
        panic!(
            "\n\
             GENERATED OUTPUT CHANGED — `generate` over the fixture no longer matches the golden.\n\
             {}\n\
             Golden:  {}\n\
             Actual:  {}\n\
             Accept with: UPDATE_SNAPSHOTS=1 cargo test -p surrealguard-codegen --test golden\n\
             then run:    pnpm -r run typecheck   (the golden is compiled by tsc there)\n",
            first_difference(&expected, &actual),
            path.display(),
            actual_path.display()
        );
    }
}

/// The fixture must be valid input. `generate` refuses to write a registry
/// when an embedded query has an error finding, and a golden produced under a
/// schema error would record types inferred against a broken schema — so the
/// bar here is stricter than the CLI's: no error finding anywhere.
#[test]
fn fixture_is_free_of_error_findings() {
    let root = fixture_root();
    let (workspace, _, config) = load(&root);
    let analysis = analyze_workspace(&workspace);
    let policy = config.policy();
    let errors: Vec<String> = analysis
        .diagnostics
        .iter()
        .filter(|finding| {
            policy.resolve_severity(finding.code(), finding.severity()) == Some(Severity::Error)
        })
        .map(|finding| {
            format!(
                "  {} {} in {}",
                surrealguard_diagnostics::render_code(finding.code(), finding.severity()),
                finding.message(),
                finding.span().source().as_str()
            )
        })
        .collect();
    assert!(
        errors.is_empty(),
        "the typecheck fixture must analyze clean, but it reported {} error finding(s):\n{}",
        errors.len(),
        errors.join("\n")
    );
}

/// Every embedded query the fixture declares lands in the golden. A query the
/// extractor silently dropped would shrink the registry without failing the
/// byte comparison on first generation, so the count is pinned separately.
#[test]
fn every_embedded_query_reaches_the_registry() {
    let root = fixture_root();
    let (_, queries, _) = load(&root);
    let module = generate(&root);
    for (_, query, _) in &queries {
        let key = query.parts().join("${}");
        assert!(
            module.contains(&format!("    \"{}\": {{", key.replace('"', "\\\""))),
            "embedded query is missing from the generated registry: {key}"
        );
    }
    assert!(
        queries.len() >= 15,
        "the fixture host file should carry the whole spread of query forms, found {}",
        queries.len()
    );
}

/// One embedded query: the virtual source it was registered under, the
/// extraction (for its template parts), and the host file it came from.
type Embedded = (
    surrealguard_syntax::source::SourceId,
    surrealguard_embed::EmbeddedQuery,
    String,
);

/// Runs `generate` over `root` and returns the rendered module — the CLI's
/// `run_generate` minus the file write and the terminal rendering.
fn generate(root: &Path) -> String {
    let (workspace, queries, _) = load(root);
    let analysis = analyze_workspace(&workspace);
    let entries: Vec<QueryEntry> = queries
        .iter()
        .filter_map(|(source_id, query, _)| {
            let output = analysis.sources.get(source_id)?;
            Some(QueryEntry::from_analysis(query.parts(), output))
        })
        .collect();
    render_registry(&entries)
}

/// Loads the fixture the way the CLI loads a workspace: config from
/// `surrealguard.toml`, `.surql` files in sorted path order with schema
/// sources (those matching a `[sources] schema` glob) registered first, then
/// every host file's embedded queries as virtual sources.
///
/// Paths are registered relative to `root` so source ids do not depend on
/// where the repository is checked out. Nothing in the output names a source
/// id, but a deterministic registration order is what keeps the golden stable.
fn load(root: &Path) -> (Workspace, Vec<Embedded>, WorkspaceConfig) {
    let config_text =
        std::fs::read_to_string(root.join("surrealguard.toml")).expect("fixture surrealguard.toml");
    let config = WorkspaceConfig::from_toml_str(&config_text).expect("fixture config parses");
    let mut workspace = Workspace::new(config.clone());

    let files = walk(root);
    let (schema, queries): (Vec<&PathBuf>, Vec<&PathBuf>) = files
        .iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "surql")
        })
        .partition(|path| matches_any_glob(root, path, &config.sources.schema));
    for path in schema.into_iter().chain(queries) {
        let text = read(path);
        workspace.add_virtual_source(relative(root, path), text);
    }

    let mut embedded = Vec::new();
    for path in files.iter().filter(|path| is_host_source(path)) {
        let text = read(path);
        let host_id = relative(root, path);
        for (index, query) in surrealguard_embed::extract(&host_id, &text)
            .into_iter()
            .enumerate()
        {
            let source_id = workspace
                .add_virtual_source(format!("embedded://{host_id}#{index}"), query.text.clone());
            embedded.push((source_id, query, host_id.clone()));
        }
    }

    (workspace, embedded, config)
}

/// Every file under `root`, in sorted path order — the order the CLI's
/// directory walk yields after its own sort.
fn walk(root: &Path) -> Vec<PathBuf> {
    fn visit(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read fixture dir") {
            let path = entry.expect("fixture dir entry").path();
            if path.is_dir() {
                visit(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    visit(root, &mut files);
    files.sort();
    files
}

/// The CLI's host-file extension set (`is_host_source` in `crates/cli`).
fn is_host_source(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("ts" | "tsx" | "js" | "jsx" | "svelte" | "vue" | "astro")
    )
}

fn matches_any_glob(root: &Path, path: &Path, globs: &[String]) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    globs
        .iter()
        .any(|glob| glob::Pattern::new(glob).is_ok_and(|pattern| pattern.matches_path(relative)))
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("fixture file under the fixture root")
        .to_string_lossy()
        .replace('\\', "/")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// The first line that differs, with its line number: enough to see *where*
/// the output moved. The full actual file is written beside the message.
fn first_difference(expected: &str, actual: &str) -> String {
    let mut expected_lines = expected.lines();
    let mut actual_lines = actual.lines();
    let mut line = 1;
    loop {
        match (expected_lines.next(), actual_lines.next()) {
            (Some(old), Some(new)) if old == new => line += 1,
            (None, None) => {
                return "(no line differences — trailing whitespace or newline only)".into()
            }
            (old, new) => {
                return format!(
                    "line {line}:\n- {}\n+ {}",
                    old.unwrap_or("<end of golden>"),
                    new.unwrap_or("<end of output>")
                );
            }
        }
    }
}
