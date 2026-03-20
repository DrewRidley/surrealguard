//! LET statement analysis.

use tree_sitter::Node;
use crate::context::Context;
use crate::types::Kind;

/// Analyze a LET statement. Binds the variable in scope.
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    // TODO: implement
    Kind::Null
}
