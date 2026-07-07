//! Expressions, idioms (field/graph paths), and literals.
//!
//! The idiom model is informed by SurrealDB's own `Part` enum (the ground
//! truth for which path constructs exist): idioms may start from a value,
//! indices are expressions, graph steps are mini-selections, and method
//! calls are path parts. We model the subset analysis consumes and keep the
//! rest explicit via `Partial`.

use super::{Block, PartialNode, Spanned, Statement, TypeExpr};

/// A value-producing expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Literal(Literal),
    /// A field path / graph traversal, rooted in the current row context or
    /// in a leading value (see [`IdiomPart::Start`]).
    Idiom(Idiom),
    /// A bare `$name` — parameter or `LET` binding reference (without `$`).
    /// `$name.field` chains lower as `Idiom` with a `Start` part instead.
    Param(String),
    /// A table reference. Only produced in source/target positions
    /// (`FROM person`, `CREATE person`): a bare identifier in expression
    /// position is a field path, not a table.
    Table(Spanned<String>),
    /// A record id literal: `person:one`. The id's internal structure is
    /// kept opaque (ids can be composite; nothing consumes their shape).
    RecordId {
        table: Spanned<String>,
        id: crate::span::ByteRange,
    },
    Binary {
        lhs: Box<Spanned<Expr>>,
        op: Spanned<BinaryOp>,
        rhs: Box<Spanned<Expr>>,
    },
    Prefix {
        op: Spanned<PrefixOp>,
        expr: Box<Spanned<Expr>>,
    },
    Call(Call),
    Object(Vec<(Spanned<String>, Spanned<Expr>)>),
    Array(Vec<Spanned<Expr>>),
    Subquery(Box<Spanned<Statement>>),
    Block(Block),
    Cast {
        ty: Spanned<TypeExpr>,
        expr: Box<Spanned<Expr>>,
    },
    Closure(Closure),
    Partial(PartialNode),
}

/// A closure value: `|$x: int| $x + 1` / `|$x| -> int { ... }`.
#[derive(Clone, Debug, PartialEq)]
pub struct Closure {
    /// Parameters with their declared types, if any.
    pub params: Vec<(Spanned<String>, Option<Spanned<TypeExpr>>)>,
    /// The declared return type (`-> int`), if any.
    pub return_ty: Option<Spanned<TypeExpr>>,
    /// The body — a block lowers to [`Expr::Block`].
    pub body: Box<Spanned<Expr>>,
}

/// A literal value. Carries only what analysis consumes: the `Int` payload
/// feeds `LIMIT`/array-length facts; the rest matter for their *kind*.
/// Datetime/Uuid/Regex arise from prefixed strings (`d'…'`/`u'…'`/`r'…'`),
/// normalized during lowering.
#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Int(i64),
    Float(f64),
    Decimal,
    String(String),
    Bool(bool),
    None,
    Null,
    Duration,
    Datetime,
    Uuid,
    Regex,
}

/// A dotted / graph path: `profile.email`, `->likes->post.{title, id}`,
/// `$user.name`, `tags[WHERE active]`. The load-bearing type of the AST —
/// every part is individually spanned so diagnostics can point at one arrow
/// or one field.
#[derive(Clone, Debug, PartialEq)]
pub struct Idiom {
    pub parts: Vec<Spanned<IdiomPart>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum IdiomPart {
    /// A leading value the rest of the path is applied to: `$user.name`,
    /// `(SELECT ...)[0]`. Always the first part when present.
    Start(Box<Spanned<Expr>>),
    /// A plain field segment: `name`.
    Field(String),
    /// Index by expression: `tags[0]`, `tags[$i]`.
    Index(Box<Spanned<Expr>>),
    /// `.*` / `[*]`.
    All,
    /// `[$]` — last element.
    Last,
    /// One graph step: `->likes` / `<-likes` / `<->likes`, possibly a full
    /// inline selection (`->(likes WHERE since > $x)`).
    Graph {
        /// Direction, spanned to the arrow token itself.
        dir: Spanned<GraphDir>,
        step: GraphStep,
    },
    /// Brace selection: `.{name, age}` — real sub-idioms, not comma-split text.
    Destructure(Vec<Spanned<Idiom>>),
    /// Inline filter: `[WHERE ...]`.
    Where(Box<Spanned<Expr>>),
    /// Method call as a path part: `foo.len()`.
    Method {
        name: Spanned<String>,
        args: Vec<Spanned<Expr>>,
    },
    /// Optional chaining marker: `foo?.bar`.
    Optional,
    Partial(PartialNode),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphDir {
    Out,
    In,
    Both,
}

/// The selection inside one graph step. SurrealDB allows a full mini-select
/// here (multiple edge tables, WHERE, ORDER, LIMIT, alias, ...) — we model
/// what analysis consumes (targets + filter) and bucket the rest.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphStep {
    /// One or more edge tables: `->likes` or `->(likes, follows)`.
    pub targets: Vec<Spanned<String>>,
    /// `->(likes WHERE since > $x)` / `->likes[WHERE ...]`.
    pub where_clause: Option<Box<Spanned<Expr>>>,
}

/// A function call. `path` is pre-normalized during lowering
/// (`type::is::record` → `type::is_record`); argument expressions keep their
/// own spans so per-argument diagnostics need no re-derivation.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub path: Spanned<String>,
    pub args: Vec<Spanned<Expr>>,
}

/// Binary operators the analyzers understand, plus an explicit escape hatch
/// for everything else — an unknown operator is a fact, not a guess.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
    NullCoalesce,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrefixOp {
    Not,
    Neg,
    Pos,
    Other(String),
}
