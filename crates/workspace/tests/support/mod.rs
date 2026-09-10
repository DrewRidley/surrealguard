//! Shared plumbing for the quality harness: loads the vendored corpus, runs
//! whole-workspace analysis over it, and enumerates every *site* that carries
//! an inferred kind.
//!
//! A "site" is one place inference produced a type: a schema field, a function
//! return, a statement's response, a `LET`/`FOR` binding, or a host param. The
//! precision snapshot renders every site; the `any` ratchet counts the `Kind::Any`
//! leaves inside each one. Both read the same enumeration so they can never
//! disagree about what the corpus contains.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use surrealdb_types::{Kind, KindLiteral};
use surrealguard_diagnostics::Finding;
use surrealguard_syntax::source::SourceId;
use surrealguard_workspace::{analyze_workspace, render_kind, Workspace, WorkspaceAnalysis};

/// The vendored corpus root. Self-contained and committed on purpose: the
/// realistic input lives outside this repo and is edited by hand, so it can
/// never back a committed snapshot.
pub fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

/// Where a golden file lives.
pub fn snapshot_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(name)
}

/// True when the harness was asked to rewrite its golden files.
pub fn updating() -> bool {
    std::env::var_os("UPDATE_SNAPSHOTS").is_some()
}

/// Every corpus `.surql` file as `(relative path, text)`, schema sources
/// first and each group in path order — a fixed registration order keeps
/// source ids, and therefore the snapshot, stable.
pub fn corpus_files() -> Vec<(String, String)> {
    let root = corpus_root();
    let mut schema = read_dir_sorted(&root.join("schema"));
    let queries = read_dir_sorted(&root.join("queries"));
    schema.extend(queries);
    assert!(
        !schema.is_empty(),
        "corpus is empty at {} — the harness would vacuously pass",
        root.display()
    );
    schema
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(&root)
                .expect("corpus entry under the corpus root")
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            (relative, text)
        })
        .collect()
}

fn read_dir_sorted(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "surql")
        })
        .collect();
    paths.sort();
    paths
}

/// The corpus loaded into a workspace and analyzed as one unit.
pub struct Corpus {
    /// The workspace the corpus was registered into (holds the line index).
    pub workspace: Workspace,
    /// Whole-workspace analysis output.
    pub analysis: WorkspaceAnalysis,
    /// Corpus-relative path per registered source, in registration order.
    pub sources: Vec<(SourceId, String)>,
}

/// Registers every corpus file and analyzes the whole set together.
pub fn analyze_corpus() -> Corpus {
    let mut workspace = Workspace::default();
    let mut sources = Vec::new();
    for (relative, text) in corpus_files() {
        let id = workspace.add_file_source(PathBuf::from(&relative), text);
        sources.push((id, relative));
    }
    let analysis = analyze_workspace(&workspace);
    Corpus {
        workspace,
        analysis,
        sources,
    }
}

impl Corpus {
    /// `line:col` (1-based) for a span, for human-readable snapshot lines.
    pub fn position(&self, span: &surrealguard_syntax::span::SourceSpan) -> String {
        let Some(index) = self.workspace.registry().line_index(span.source()) else {
            return "?:?".to_string();
        };
        let at = index.line_column(span.range().start());
        format!("{}:{}", at.line + 1, at.column + 1)
    }
}

/// One inferred type in the corpus, with a stable identity.
pub struct Site {
    /// Stable, sortable identity — `schema/field/organization.name`,
    /// `queries/40_bindings.surql/let/$orgs@5:5`, ... Used as the ratchet key,
    /// so it must not embed anything that churns for unrelated reasons.
    pub id: String,
    /// The kind inference produced, `None` when it produced nothing at all.
    pub kind: Option<Kind>,
}

impl Site {
    /// The kind as the editor would render it (`unknown` is how the surfaces
    /// spell an absent kind; `any` is how they spell `Kind::Any`).
    pub fn rendered(&self) -> String {
        match &self.kind {
            Some(kind) => render_kind(kind),
            None => "unknown".to_string(),
        }
    }

    /// How many `Kind::Any` leaves the site's kind contains — 1 for a bare
    /// `any`, N for `any` nested N times inside a container or object. A site
    /// with no kind at all counts as one unknown.
    pub fn any_count(&self) -> usize {
        match &self.kind {
            Some(kind) => count_any(kind),
            None => 1,
        }
    }
}

/// Counts `Kind::Any` leaves anywhere inside `kind`.
pub fn count_any(kind: &Kind) -> usize {
    match kind {
        Kind::Any => 1,
        Kind::Array(inner, _) | Kind::Set(inner, _) => count_any(inner),
        Kind::Either(variants) => variants.iter().map(count_any).sum(),
        Kind::Literal(KindLiteral::Array(kinds)) => kinds.iter().map(count_any).sum(),
        Kind::Literal(KindLiteral::Object(entries)) => entries.values().map(count_any).sum(),
        _ => 0,
    }
}

/// Every site in the corpus, in a deterministic order: schema first (fields,
/// then function returns), then each source's statements, bindings and params
/// in registration order.
pub fn sites(corpus: &Corpus) -> Vec<Site> {
    let mut sites = Vec::new();

    for (table_name, table) in &corpus.analysis.schema.tables {
        for (path, field) in &table.fields {
            sites.push(Site {
                id: format!("schema/field/{table_name}.{path}"),
                kind: field.kind.clone(),
            });
        }
    }
    for (name, function) in &corpus.analysis.schema.functions {
        sites.push(Site {
            id: format!("schema/fn/{name}"),
            kind: function
                .return_kind
                .clone()
                .or_else(|| function.inferred_return.clone()),
        });
    }

    for (source, relative) in &corpus.sources {
        let Some(output) = corpus.analysis.sources.get(source) else {
            continue;
        };
        for statement in &output.statements {
            // Only responding statements carry a response kind; a `DEFINE`
            // legitimately has none, so it is not a site.
            if statement.response_kind.is_none() {
                continue;
            }
            sites.push(Site {
                id: format!(
                    "{relative}/stmt/{}@{}",
                    statement.kind,
                    corpus.position(&statement.span)
                ),
                kind: statement.response_kind.clone(),
            });
        }
        for binding in &output.let_bindings {
            sites.push(Site {
                id: format!(
                    "{relative}/let/${}@{}",
                    binding.name,
                    corpus.position(&binding.name_span)
                ),
                kind: binding.kind.clone(),
            });
        }
        let mut params: Vec<_> = output.inferred_params.iter().collect();
        params.sort_by(|a, b| a.name.cmp(&b.name));
        for param in params {
            sites.push(Site {
                id: format!("{relative}/param/${}", param.name),
                kind: param.kind.clone(),
            });
        }
    }

    sites
}

/// A minimal line diff (LCS) rendered as a unified-ish hunk list, so a failing
/// golden file tells you *what* moved rather than dumping both versions.
pub fn diff_lines(expected: &str, actual: &str) -> String {
    let old: Vec<&str> = expected.lines().collect();
    let new: Vec<&str> = actual.lines().collect();

    // lcs[i][j] = length of the longest common subsequence of old[i..], new[j..].
    let mut lcs = vec![vec![0usize; new.len() + 1]; old.len() + 1];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    // Walk the LCS into a flat op list first, then decide what to print.
    let mut ops: Vec<(char, &str)> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < old.len() || j < new.len() {
        if i < old.len() && j < new.len() && old[i] == new[j] {
            ops.push((' ', old[i]));
            i += 1;
            j += 1;
        } else if j < new.len() && (i >= old.len() || lcs[i][j + 1] >= lcs[i + 1][j]) {
            ops.push(('+', new[j]));
            j += 1;
        } else {
            ops.push(('-', old[i]));
            i += 1;
        }
    }

    // Print every changed line plus `CONTEXT` unchanged lines either side,
    // eliding the runs in between so a large golden file stays readable.
    const CONTEXT: usize = 2;
    let mut keep = vec![false; ops.len()];
    for (index, (tag, _)) in ops.iter().enumerate() {
        if *tag == ' ' {
            continue;
        }
        let start = index.saturating_sub(CONTEXT);
        let end = (index + CONTEXT + 1).min(ops.len());
        for slot in &mut keep[start..end] {
            *slot = true;
        }
    }

    let mut out = String::new();
    let mut elided = false;
    for (index, (tag, line)) in ops.iter().enumerate() {
        if keep[index] {
            out.push_str(&format!("{tag} {line}\n"));
            elided = false;
        } else if !elided {
            out.push_str("  ...\n");
            elided = true;
        }
    }
    if !ops.iter().any(|(tag, _)| *tag != ' ') {
        out.push_str("(no line differences — trailing whitespace or newline only)\n");
    }
    out
}

/// How many findings with catalog number `number` a whole-workspace pass
/// raised, across every source.
pub fn codes(output: &WorkspaceAnalysis, number: u16) -> usize {
    output
        .diagnostics
        .iter()
        .filter(|finding| finding.code().number() == number)
        .count()
}

/// A parsed fixture must reach semantic analysis: any `S`-category
/// finding means a syntax error short-circuited the pipeline, so a
/// semantic assertion that follows would be vacuous.
pub fn assert_no_syntax_findings(diagnostics: &[Finding]) {
    assert!(
        !diagnostics
            .iter()
            .any(|finding| finding.code().to_string().starts_with('S')),
        "fixture failed to parse: {:?}",
        diagnostics
            .iter()
            .map(|f| (f.code().to_string(), f.message().to_string()))
            .collect::<Vec<_>>()
    );
}
