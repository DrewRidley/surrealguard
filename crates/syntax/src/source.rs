//! Source identity primitives.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable identity for a source analyzed by SurrealGuard.
///
/// This may be a real file URI, an embedded-query virtual URI, or any adapter-owned
/// identifier. Spans should never travel without a source id.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SourceId(String);

impl SourceId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for SourceId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for SourceId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_id_preserves_input_text() {
        let source = SourceId::new("file:///schema.surql");

        assert_eq!(source.as_str(), "file:///schema.surql");
        assert_eq!(source.to_string(), "file:///schema.surql");
    }
}
