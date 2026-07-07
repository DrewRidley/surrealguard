//! Typed, span-carrying SurrealQL AST.
//!
//! Produced by [`crate::lower`] from the tree-sitter CST; consumed by
//! analyzers. The contract:
//!
//! - **Purely syntactic.** No name resolution, no schema, no environment.
//! - **Spans on everything.** Every node carries the byte range of the CST
//!   node it came from; nothing holds a `tree_sitter::Node` or a lifetime.
//! - **Partiality is explicit.** Enums carry a `Partial(PartialNode)` variant
//!   for positions that failed to lower; statement structs carry an
//!   `unhandled` bucket for CST children the lowering did not consume.
//!   Nothing is silently dropped.
//! - **Normalized.** Keywords are case-folded, function paths canonicalized,
//!   grammar quirks flattened — before analyzers ever see the tree.

mod clause;
mod expr;
mod statement;
mod ty;

pub use clause::*;
pub use expr::*;
pub use statement::*;
pub use ty::*;

use crate::span::ByteRange;

/// A syntax node paired with the byte range it was lowered from.
///
/// `SourceId` travels separately (analysis knows which source it is
/// working on); spans are plain `Copy` byte ranges so the AST is `'static`.
#[derive(Clone, Debug, PartialEq)]
pub struct Spanned<T> {
    pub node: T,
    pub span: ByteRange,
}

impl<T> Spanned<T> {
    pub fn new(node: T, span: ByteRange) -> Self {
        Self { node, span }
    }

    /// Maps the inner node, keeping the span.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Spanned<U> {
        Spanned {
            node: f(self.node),
            span: self.span,
        }
    }
}

/// A region of syntax the lowering could not (or does not) model:
/// a CST `ERROR`/`MISSING` node, or a construct outside the modeled tier.
///
/// Analyzers translate these into explicit partial analysis facts — they
/// must never be ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartialNode {
    pub span: ByteRange,
    /// The CST node kind this region had (`"ERROR"`, `"MISSING Ident"`, or
    /// the named kind of an unmodeled construct).
    pub cst_kind: String,
}
