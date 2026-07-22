//! Byte-based span primitives.

use serde::{Deserialize, Serialize};

use crate::source::SourceId;

/// Half-open byte range, `[start, end)`, in a source file or virtual source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ByteRange {
    start: u32,
    end: u32,
}

impl ByteRange {
    /// Builds a range, rejecting `start > end`.
    pub fn new(start: u32, end: u32) -> Result<Self, InvalidByteRange> {
        if start <= end {
            Ok(Self { start, end })
        } else {
            Err(InvalidByteRange { start, end })
        }
    }

    /// Inclusive start byte offset.
    pub fn start(self) -> u32 {
        self.start
    }

    /// Exclusive end byte offset.
    pub fn end(self) -> u32 {
        self.end
    }

    /// Number of bytes covered.
    pub fn len(self) -> u32 {
        self.end - self.start
    }

    /// Whether the range covers zero bytes.
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// Error returned when constructing a byte range with `start > end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidByteRange {
    start: u32,
    end: u32,
}

impl InvalidByteRange {
    /// The rejected start offset.
    pub fn start(self) -> u32 {
        self.start
    }

    /// The rejected end offset.
    pub fn end(self) -> u32 {
        self.end
    }
}

/// A byte range tied to the source it belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceSpan {
    source: SourceId,
    range: ByteRange,
}

impl SourceSpan {
    /// Pairs a byte range with the source it belongs to.
    pub fn new(source: SourceId, range: ByteRange) -> Self {
        Self { source, range }
    }

    /// The source this span points into.
    pub fn source(&self) -> &SourceId {
        &self.source
    }

    /// The byte range within the source.
    pub fn range(&self) -> ByteRange {
        self.range
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceId;

    #[test]
    fn byte_range_accepts_ordered_bounds() {
        let range = ByteRange::new(0, 3).expect("ordered range should be valid");

        assert_eq!(range.start(), 0);
        assert_eq!(range.end(), 3);
        assert_eq!(range.len(), 3);
        assert!(!range.is_empty());
    }

    #[test]
    fn byte_range_rejects_reversed_bounds() {
        let error = ByteRange::new(3, 0).expect_err("reversed range should be invalid");

        assert_eq!(error.start(), 3);
        assert_eq!(error.end(), 0);
    }

    #[test]
    fn source_span_preserves_source_identity_and_range() {
        let source = SourceId::new("query:001");
        let range = ByteRange::new(4, 9).expect("range should be valid");

        let span = SourceSpan::new(source.clone(), range);

        assert_eq!(span.source(), &source);
        assert_eq!(span.range(), range);
    }
}
