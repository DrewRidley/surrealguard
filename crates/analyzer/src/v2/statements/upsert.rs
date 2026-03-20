//! UPSERT statement analysis.

use tree_sitter::Node;
use crate::context::Context;
use crate::types::Kind;

/// Analyze a UPSERT statement. Returns the result type.
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    // TODO: implement
    Kind::Any
}
