//! Syntactic type expressions (`TYPE array<string>`, `option<int | string>`).
//!
//! These are *syntax*, deliberately independent of `surrealdb_types::Kind` —
//! conversion to `Kind` is an analysis concern and lives in the workspace
//! crate. Modeling this structurally closes the long-standing gap where
//! `array<string>` fields degraded to `UnsupportedSyntax`.

use super::{PartialNode, Spanned};

#[derive(Clone, Debug, PartialEq)]
pub enum TypeExpr {
    /// `string`, `int`, `record`, ...
    Name(Spanned<String>),
    /// `array<string>`, `record<user>`, `set<int, 5>`, ...
    Parameterized {
        name: Spanned<String>,
        args: Vec<Spanned<TypeExpr>>,
    },
    /// `int | string`
    Union(Vec<Spanned<TypeExpr>>),
    /// `option<...>` sugar and the grammar's optional marker.
    Optional(Box<Spanned<TypeExpr>>),
    /// A literal type: `'active'`, `42`, `true` — usually inside unions.
    Literal(crate::ast::Literal),
    /// Object types and other unmodeled type syntax: explicit, never dropped.
    Partial(PartialNode),
}
