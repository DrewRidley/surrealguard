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
    /// A literal value.
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
        /// The table portion (`person` in `person:one`).
        table: Spanned<String>,
        /// The id portion, kept as an opaque span.
        id: crate::span::ByteRange,
    },
    /// A binary operation: `a + b`, `a AND b`.
    Binary {
        /// Left operand.
        lhs: Box<Spanned<Expr>>,
        /// The operator.
        op: Spanned<BinaryOp>,
        /// Right operand.
        rhs: Box<Spanned<Expr>>,
    },
    /// A prefix operation: `!x`, `-x`.
    Prefix {
        /// The operator.
        op: Spanned<PrefixOp>,
        /// The operand.
        expr: Box<Spanned<Expr>>,
    },
    /// A function call.
    Call(Call),
    /// An object literal: `{ a: 1, b: 2 }`.
    Object(Vec<(Spanned<String>, Spanned<Expr>)>),
    /// An array literal: `[1, 2, 3]`.
    Array(Vec<Spanned<Expr>>),
    /// A parenthesized statement used as a value: `(SELECT ...)`.
    Subquery(Box<Spanned<Statement>>),
    /// A `{ ...; ... }` block used as a value.
    Block(Block),
    /// A type cast: `<int> $x`.
    Cast {
        /// The target type.
        ty: Spanned<TypeExpr>,
        /// The expression being cast.
        expr: Box<Spanned<Expr>>,
    },
    /// A closure value.
    Closure(Closure),
    /// An expression that failed to lower.
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
    /// An integer literal; its value feeds `LIMIT`/array-length facts.
    Int(i64),
    /// A floating-point literal.
    Float(f64),
    /// A `decimal` literal — only its kind matters, so no value is kept.
    Decimal,
    /// A string literal's contents.
    String(String),
    /// A boolean literal.
    Bool(bool),
    /// The `NONE` literal.
    None,
    /// The `NULL` literal.
    Null,
    /// A duration literal, carrying its raw text (`1w2d`).
    Duration(String),
    /// A datetime literal's inner text (`2024-01-01T00:00:00Z`).
    Datetime(String),
    /// A uuid literal's inner text.
    Uuid(String),
    /// A regex literal's pattern text.
    Regex(String),
}

/// A dotted / graph path: `profile.email`, `->likes->post.{title, id}`,
/// `$user.name`, `tags[WHERE active]`. The load-bearing type of the AST —
/// every part is individually spanned so diagnostics can point at one arrow
/// or one field.
#[derive(Clone, Debug, PartialEq)]
pub struct Idiom {
    /// The path segments, in order; each is individually spanned.
    pub parts: Vec<Spanned<IdiomPart>>,
}

/// One segment of an [`Idiom`] path.
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
        /// The edge selection reached by the step.
        step: GraphStep,
    },
    /// Brace selection: `.{name, age}` — real sub-idioms, not comma-split text.
    Destructure(Vec<Spanned<Idiom>>),
    /// Inline filter: `[WHERE ...]`.
    Where(Box<Spanned<Expr>>),
    /// Method call as a path part: `foo.len()`.
    Method {
        /// The method name.
        name: Spanned<String>,
        /// The call arguments.
        args: Vec<Spanned<Expr>>,
    },
    /// Graph recursion: `.{1..3}` / `.{..}` — `bounded` is whether an
    /// upper bound was written.
    Recurse {
        /// Whether an upper recursion bound was written.
        bounded: bool,
    },
    /// Optional chaining marker: `foo?.bar`.
    Optional,
    /// A path segment that failed to lower.
    Partial(PartialNode),
}

/// Direction of a graph traversal step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphDir {
    /// `->` — outgoing edges.
    Out,
    /// `<-` — incoming edges.
    In,
    /// `<->` — edges in either direction.
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
    /// The canonicalized function path (`type::is_record`).
    pub path: Spanned<String>,
    /// The argument expressions, each retaining its own span.
    pub args: Vec<Spanned<Expr>>,
}

/// Binary operators the analyzers understand, plus an explicit escape hatch
/// for everything else — an unknown operator is a fact, not a guess.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `=` / `==`
    Eq,
    /// `!=`
    NotEq,
    /// `<`
    Lt,
    /// `<=`
    LtEq,
    /// `>`
    Gt,
    /// `>=`
    GtEq,
    /// `AND` / `&&`
    And,
    /// `OR` / `||`
    Or,
    /// `??` — null coalescing.
    NullCoalesce,
    /// Any operator not modeled above, kept as raw text.
    Other(String),
}

/// Prefix operators, with an explicit escape hatch for the unmodeled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrefixOp {
    /// `!` — logical negation.
    Not,
    /// `-` — arithmetic negation.
    Neg,
    /// `+` — unary plus.
    Pos,
    /// Any operator not modeled above, kept as raw text.
    Other(String),
}
