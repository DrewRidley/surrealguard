//! Maps workspace files and virtual sources to parsed, identified sources.

use std::collections::BTreeMap;
use std::path::PathBuf;

use surrealguard_syntax::source::SourceId;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceRegistry {
    sources: BTreeMap<SourceId, RegisteredSource>,
    file_ids: BTreeMap<PathBuf, SourceId>,
    next_virtual_id: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredSource {
    id: SourceId,
    text: String,
    line_index: LineIndex,
}

impl RegisteredSource {
    pub fn id(&self) -> &SourceId {
        &self.id
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn line_index(&self) -> &LineIndex {
        &self.line_index
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineIndex {
    line_starts: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineColumn {
    pub line: u32,
    pub column: u32,
}

impl SourceRegistry {
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
        source_id
    }

    pub fn add_virtual(&mut self, name: String, text: String) -> SourceId {
        let source_id = SourceId::new(format!("virtual://{}#{}", name, self.next_virtual_id));
        self.next_virtual_id += 1;
        self.sources.insert(
            source_id.clone(),
            RegisteredSource::new(source_id.clone(), text),
        );
        source_id
    }

    pub fn source(&self, source: &SourceId) -> Option<&RegisteredSource> {
        self.sources.get(source)
    }

    pub fn text(&self, source: &SourceId) -> Option<&str> {
        self.source(source).map(RegisteredSource::text)
    }

    pub fn line_index(&self, source: &SourceId) -> Option<&LineIndex> {
        self.source(source).map(RegisteredSource::line_index)
    }

    pub fn source_ids(&self) -> impl Iterator<Item = &SourceId> {
        self.sources.keys()
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
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0];

        for (byte_index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push((byte_index + 1) as u32);
            }
        }

        Self { line_starts }
    }

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
