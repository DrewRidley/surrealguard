//! Clauses shared across statement kinds.

use super::{Expr, Idiom, PartialNode, Spanned};
use crate::span::ByteRange;

/// One output column of a SELECT projection list or a mutation
/// `RETURN <fields>` list.
#[derive(Clone, Debug, PartialEq)]
pub enum Projection {
    /// `*`
    Wildcard(ByteRange),
    Expr {
        expr: Spanned<Expr>,
        alias: Option<Spanned<String>>,
    },
    Partial(PartialNode),
}

/// A mutation's `RETURN` clause, parsed — replaces substring classification.
#[derive(Clone, Debug, PartialEq)]
pub enum ReturnMode {
    None,
    Null,
    Diff,
    Before,
    After,
    Fields(Vec<Projection>),
}

/// A mutation's payload clause.
#[derive(Clone, Debug, PartialEq)]
pub enum DataClause {
    Set(Vec<Assignment>),
    Unset(Vec<Spanned<Idiom>>),
    Content(Spanned<Expr>),
    Merge(Spanned<Expr>),
    Patch(Spanned<Expr>),
    Replace(Spanned<Expr>),
    /// A bare value payload (e.g. `INSERT INTO t $object`).
    Single(Spanned<Expr>),
    Partial(PartialNode),
}

/// `target op value` inside a SET clause.
#[derive(Clone, Debug, PartialEq)]
pub struct Assignment {
    pub target: Spanned<Idiom>,
    pub op: Spanned<AssignOp>,
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
    Other(String),
}

/// `ORDER BY` — one or more sort keys.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderClause {
    pub keys: Vec<OrderKey>,
}

/// One `ORDER BY` key and its direction.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderKey {
    pub expr: Spanned<Expr>,
    pub descending: bool,
}

/// `GROUP BY <keys>` / `GROUP ALL`.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupClause {
    /// `GROUP ALL` (aggregate to a single row) vs `GROUP BY <keys>`.
    pub all: bool,
    pub keys: Vec<Spanned<Idiom>>,
}
