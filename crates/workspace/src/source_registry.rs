//! Maps workspace files and virtual sources to parsed, identified sources.

use std::collections::BTreeMap;
use std::path::PathBuf;

use surrealguard_syntax::source::SourceId;

/// The set of sources analysis runs over, keyed by [`SourceId`]. File
/// sources keep a stable id across edits; virtual sources get a fresh id
/// each time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceRegistry {
    sources: BTreeMap<SourceId, RegisteredSource>,
    file_ids: BTreeMap<PathBuf, SourceId>,
    next_virtual_id: u64,
    /// Sources in the order they were added. Analysis walks sources in this
    /// order and accumulates the schema as it goes, so a caller can make
    /// schema sources visible to later query sources by adding them first —
    /// insertion order, not id-sort order, is what analysis honors.
    order: Vec<SourceId>,
}

/// One registered source: its id, full text, and a precomputed line index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredSource {
    id: SourceId,
    text: String,
    line_index: LineIndex,
}

impl RegisteredSource {
    /// The source's stable identifier.
    pub fn id(&self) -> &SourceId {
        &self.id
    }

    /// The full source text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The line index for mapping byte offsets to line/column.
    pub fn line_index(&self) -> &LineIndex {
        &self.line_index
    }
}

/// Byte offsets of each line start, for resolving spans to line/column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineIndex {
    line_starts: Vec<u32>,
}

/// A zero-based line and column position within a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineColumn {
    /// Zero-based line number.
    pub line: u32,
    /// Zero-based column, counted in bytes from the line start.
    pub column: u32,
}

impl SourceRegistry {
    /// Registers or updates a file source. A path already registered keeps
    /// its [`SourceId`] and has its text replaced.
    pub fn add_file(&mut self, path: PathBuf, text: String) -> SourceId {
        if let Some(source_id) = self.file_ids.get(&path).cloned() {
            self.sources.insert(
                source_id.clone(),
                RegisteredSource::new(source_id.clone(), text),
            );
            return source_id;
        }

        let source_id = SourceId::new(format!("file://{}", path.to_string_lossy()));
        self.file_ids.insert(path, source_id.clone());
        self.sources.insert(
            source_id.clone(),
            RegisteredSource::new(source_id.clone(), text),
        );
        self.order.push(source_id.clone());
        source_id
    }

    /// Registers a virtual (non-file) source under a fresh, unique
    /// [`SourceId`] derived from `name`.
    pub fn add_virtual(&mut self, name: String, text: String) -> SourceId {
        let source_id = SourceId::new(format!("virtual://{}#{}", name, self.next_virtual_id));
        self.next_virtual_id += 1;
        self.sources.insert(
            source_id.clone(),
            RegisteredSource::new(source_id.clone(), text),
        );
        self.order.push(source_id.clone());
        source_id
    }

    /// The registered source for `source`, if any.
    pub fn source(&self, source: &SourceId) -> Option<&RegisteredSource> {
        self.sources.get(source)
    }

    /// The text of `source`, if registered.
    pub fn text(&self, source: &SourceId) -> Option<&str> {
        self.source(source).map(RegisteredSource::text)
    }

    /// The line index of `source`, if registered.
    pub fn line_index(&self, source: &SourceId) -> Option<&LineIndex> {
        self.source(source).map(RegisteredSource::line_index)
    }

    /// Every registered source id, in the order sources were added. Analysis
    /// walks sources in this order, so schema sources added before query
    /// sources are visible to them.
    pub fn source_ids(&self) -> impl Iterator<Item = &SourceId> {
        self.order.iter()
    }
}

impl RegisteredSource {
    fn new(id: SourceId, text: String) -> Self {
        let line_index = LineIndex::new(&text);
        Self {
            id,
            text,
            line_index,
        }
    }
}

impl LineIndex {
    /// Builds the index by scanning `text` for line breaks.
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0];

        for (byte_index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push((byte_index + 1) as u32);
            }
        }

        Self { line_starts }
    }

    /// Resolves a byte offset to its zero-based line and column.
    pub fn line_column(&self, byte_offset: u32) -> LineColumn {
        let line_index = match self.line_starts.binary_search(&byte_offset) {
            Ok(index) => index,
            Err(0) => 0,
            Err(index) => index - 1,
        };
        let line_start = self.line_starts[line_index];

        LineColumn {
            line: line_index as u32,
            column: byte_offset.saturating_sub(line_start),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_file_path_keeps_stable_source_id_and_updates_text() {
        let mut registry = SourceRegistry::default();
        let path = PathBuf::from("schema/user.surql");

        let first = registry.add_file(path.clone(), "DEFINE TABLE user;".into());
        let second = registry.add_file(path, "DEFINE TABLE user SCHEMAFULL;".into());

        assert_eq!(first, second);
        assert_eq!(registry.text(&first), Some("DEFINE TABLE user SCHEMAFULL;"));
    }

    #[test]
    fn virtual_sources_get_unique_ids() {
        let mut registry = SourceRegistry::default();

        let first = registry.add_virtual("query".into(), "SELECT * FROM user;".into());
        let second = registry.add_virtual("query".into(), "SELECT * FROM org;".into());

        assert_ne!(first, second);
        assert_eq!(registry.text(&first), Some("SELECT * FROM user;"));
        assert_eq!(registry.text(&second), Some("SELECT * FROM org;"));
    }

    #[test]
    fn line_index_maps_byte_offsets_to_zero_based_line_and_column() {
        let index = LineIndex::new("one\ntwo\nthree");

        assert_eq!(index.line_column(0), LineColumn { line: 0, column: 0 });
        assert_eq!(index.line_column(4), LineColumn { line: 1, column: 0 });
        assert_eq!(index.line_column(6), LineColumn { line: 1, column: 2 });
        assert_eq!(index.line_column(8), LineColumn { line: 2, column: 0 });
    }
}
