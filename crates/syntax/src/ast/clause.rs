//! Clauses shared across statement kinds.

use super::{Expr, Idiom, PartialNode, Spanned};
use crate::span::ByteRange;

/// One output column of a SELECT projection list or a mutation
/// `RETURN <fields>` list.
#[derive(Clone, Debug, PartialEq)]
pub enum Projection {
    /// `*`
    Wildcard(ByteRange),
    /// A projected expression, optionally aliased with `AS`.
    Expr {
        /// The projected value expression.
        expr: Spanned<Expr>,
        /// `AS <name>` — the output column name, if renamed.
        alias: Option<Spanned<String>>,
    },
    /// A projection that failed to lower.
    Partial(PartialNode),
}

/// A mutation's `RETURN` clause.
#[derive(Clone, Debug, PartialEq)]
pub enum ReturnMode {
    /// `RETURN NONE` — suppress the result.
    None,
    /// `RETURN NULL`.
    Null,
    /// `RETURN DIFF` — the JSON-patch delta of each row.
    Diff,
    /// `RETURN BEFORE` — the row as it was before the mutation.
    Before,
    /// `RETURN AFTER` — the row after the mutation (the default).
    After,
    /// `RETURN <fields>` — an explicit projection list.
    Fields(Vec<Projection>),
}

/// A mutation's payload clause.
#[derive(Clone, Debug, PartialEq)]
pub enum DataClause {
    /// `SET a = 1, b = 2` — per-field assignments.
    Set(Vec<Assignment>),
    /// `UNSET a, b` — fields to remove.
    Unset(Vec<Spanned<Idiom>>),
    /// `CONTENT <expr>` — replace the row with the object.
    Content(Spanned<Expr>),
    /// `MERGE <expr>` — shallow-merge the object into the row.
    Merge(Spanned<Expr>),
    /// `PATCH <expr>` — apply a JSON-patch array.
    Patch(Spanned<Expr>),
    /// `REPLACE <expr>` — overwrite the row with the object.
    Replace(Spanned<Expr>),
    /// A bare value payload (e.g. `INSERT INTO t $object`).
    Single(Spanned<Expr>),
    /// A payload clause that failed to lower.
    Partial(PartialNode),
}

/// `target op value` inside a SET clause.
#[derive(Clone, Debug, PartialEq)]
pub struct Assignment {
    /// The field path being assigned to.
    pub target: Spanned<Idiom>,
    /// The assignment operator (`=`, `+=`, ...).
    pub op: Spanned<AssignOp>,
    /// The right-hand-side value expression.
    pub value: Spanned<Expr>,
}

/// The operator of a `SET` assignment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssignOp {
    /// `=`
    Assign,
    /// `+=`
    Add,
    /// `-=`
    Sub,
    /// `+?=` (add-if-missing)
    Extend,
    /// Any other operator, kept as raw text rather than guessed.
    Other(String),
}

/// `ORDER BY` — one or more sort keys.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderClause {
    /// The sort keys, in precedence order.
    pub keys: Vec<OrderKey>,
}

/// One `ORDER BY` key and its direction.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderKey {
    /// The key expression to sort on.
    pub expr: Spanned<Expr>,
    /// `DESC` — descending order, otherwise ascending.
    pub descending: bool,
}

/// `GROUP BY <keys>` / `GROUP ALL`.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupClause {
    /// `GROUP ALL` (aggregate to a single row) vs `GROUP BY <keys>`.
    pub all: bool,
    /// The grouping keys (empty for `GROUP ALL`).
    pub keys: Vec<Spanned<Idiom>>,
}
