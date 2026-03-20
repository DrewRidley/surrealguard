//! v2 analyzer — clean rewrite with self-contained statement modules.
//!
//! Each statement module handles its own analysis. No shared "resolve" god function.
//! Shared code is limited to: Context, Diagnostic, type utilities, parser helpers.

pub mod expr;
pub mod statements;

use tree_sitter::Node;
use crate::context::Context;
use crate::diagnostic::Diagnostic;
use crate::parser::{find_all, child_by_kind, node_text};
use crate::span::Span;
use crate::types::Kind;

/// Analyze all statements in a parsed tree.
pub fn analyze_all(root: &Node, source: &str, ctx: &mut Context) {
    for node in find_all(root, "select_statement") {
        statements::select::analyze(&node, source, ctx);
    }
    for node in find_all(root, "create_statement") {
        statements::create::analyze(&node, source, ctx);
    }
    for node in find_all(root, "update_statement") {
        statements::update::analyze(&node, source, ctx);
    }
    for node in find_all(root, "delete_statement") {
        statements::delete::analyze(&node, source, ctx);
    }
    for node in find_all(root, "insert_statement") {
        statements::insert::analyze(&node, source, ctx);
    }
    for node in find_all(root, "upsert_statement") {
        statements::upsert::analyze(&node, source, ctx);
    }
    for node in find_all(root, "relate_statement") {
        statements::relate::analyze(&node, source, ctx);
    }
    for node in find_all(root, "let_statement") {
        statements::let_stmt::analyze(&node, source, ctx);
    }
}
