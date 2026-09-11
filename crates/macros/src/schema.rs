//! Compile-time schema discovery for the query macros.
//!
//! Resolves SurrealQL schema sources so queries can be checked and typed
//! against real tables and fields. Resolution order:
//!
//! 1. The `SURREALQL_ANALYZER_SCHEMA` env var — a file or a directory, taken
//!    relative to `CARGO_MANIFEST_DIR` unless absolute.
//! 2. Convention paths under the crate root, first match wins: `schema/`,
//!    `migrations/`, then `schema.surql`.
//!
//! A directory contributes every `.surql`/`.surrealql` file it contains,
//! **sorted by path** — so zero-padded migrations (`0001_*.surql`,
//! `0002_*.surql`) apply in order. Nothing found → schemaless (queries are
//! still checked for everything that doesn't depend on a schema).

use std::path::{Path, PathBuf};

/// Every schema source, in application order. Empty means schemaless.
pub fn load() -> Vec<(PathBuf, String)> {
    let Some(root) = std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from) else {
        return Vec::new();
    };
    let Some(target) = resolve(&root) else {
        return Vec::new();
    };
    let mut files = collect(&target);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn resolve(root: &Path) -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("SURREALQL_ANALYZER_SCHEMA") {
        let path = PathBuf::from(configured);
        let path = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        return path.exists().then_some(path);
    }
    ["schema", "migrations", "schema.surql"]
        .into_iter()
        .map(|candidate| root.join(candidate))
        .find(|path| path.exists())
}

fn collect(target: &Path) -> Vec<(PathBuf, String)> {
    if target.is_file() {
        return read(target).into_iter().collect();
    }
    let mut out = Vec::new();
    let mut stack = vec![target.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if is_surql(&path) {
                out.extend(read(&path));
            }
        }
    }
    out
}

fn is_surql(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("surql" | "surrealql")
    )
}

fn read(path: &Path) -> Option<(PathBuf, String)> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| (path.to_path_buf(), text))
}
