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
    /// A module constant: `math::pi`, `time::EPOCH`. The path is
    /// case-folded to lowercase (`MaTh::Pi` → `math::pi`), which is how the
    /// engine resolves it.
    Constant(Spanned<String>),
    /// A record id literal: `person:one`. The id's internal structure is
    /// kept opaque (ids can be composite; nothing consumes their shape).
    RecordId {
        /// The table portion (`person` in `person:one`).
        table: Spanned<String>,
        /// The id portion, kept as an opaque span.
        id: crate::span::ByteRange,
        /// Whether the id portion is a *range* (`person:a..z`) rather than a
        /// single id. A range denotes many records, so it carries none of the
        /// single-row guarantee a plain record id does.
        range: bool,
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
    /// A range value: `1..5`, `..=10`, `$a>..$b`. Either bound may be
    /// absent; a record-id range (`person:1..5`) is an [`Expr::RecordId`]
    /// with `range` set, not this.
    Range(Range),
    /// A prefix operation: `!x`, `-x`, `NOT x`.
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

/// A range expression's two bounds and their inclusivity.
#[derive(Clone, Debug, PartialEq)]
pub struct Range {
    /// The lower bound, if written.
    pub start: Option<Box<Spanned<Expr>>>,
    /// The upper bound, if written.
    pub end: Option<Box<Spanned<Expr>>>,
    /// `>..` — the lower bound is excluded.
    pub start_exclusive: bool,
    /// `..=` — the upper bound is included.
    pub end_inclusive: bool,
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
/// Datetime/Uuid/Bytes/File arise from prefixed strings (`d'…'`/`u'…'`/
/// `b'…'`/`f'…'`), normalized during lowering; `r'…'` is a record id and
/// lowers to [`Expr::RecordId`], and a regex is only ever written `/…/`.
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
    /// A bytes literal's inner text (`b'…'`).
    Bytes(String),
    /// A file literal's inner text (`f'bucket:/path'`).
    File(String),
    /// A point literal — `(1.5, 2.5)`. The engine types this as
    /// `geometry<point>` (`RETURN type::of((1.5, 2.5))` on 3.2.3), so the
    /// longitude/latitude pair is kept as written.
    Point(f64, f64),
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
    /// Flatten marker: `tags...` — one level of nested arrays is flattened.
    Flatten,
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

/// The selection inside one graph step. SurrealDB allows a mini-select here
/// (several tables, WHERE, LIMIT/START, alias) — we model what analysis
/// consumes (targets + filter) and bucket the rest.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphStep {
    /// One or more tables the step names: `->likes`, `->(likes, follows)`,
    /// or the table a record range covers (`->(post:1..9)` → `post`).
    pub targets: Vec<Spanned<String>>,
    /// `->(likes WHERE since > $x)` / `->likes[WHERE ...]`.
    pub where_clause: Option<Box<Spanned<Expr>>>,
    /// `->(likes LIMIT 3)` — how many of the reached rows the step keeps.
    /// Narrows the traversal's cardinality, never its shape.
    pub limit: Option<Box<Spanned<Expr>>>,
    /// `->(likes START 3)`, the same clause's other half.
    pub start: Option<Box<Spanned<Expr>>>,
    /// The step is a record-reference traversal (`<~`), not a graph-edge
    /// traversal (`<-`/`->`/`<->`). Reference steps follow `REFERENCE` fields
    /// rather than relation tables, so typing resolves them differently.
    pub reference: bool,
    /// The step names `?` — every edge, whatever it is (`->?`, `->(?)`).
    /// Legal SurrealQL with no single table behind it.
    pub wildcard: bool,
    /// `->(likes AS liked)` — the key the step's result is stored under.
    pub alias: Option<Spanned<String>>,
    /// Target-position syntax the grammar admits and this AST does not model:
    /// `->(post.{title})`, `->(post:one)`, `->(post->wrote)`. SurrealDB
    /// rejects all of these outright, so nothing here names a table — but
    /// dropping them left the step looking like it named nothing at all,
    /// which reads as "resolved to nothing" instead of "not understood".
    pub unmodeled: Vec<PartialNode>,
}

/// A function call. `path` is pre-normalized during lowering
/// (`type::is::record` → `type::is_record`); argument expressions keep their
/// own spans so per-argument diagnostics need no re-derivation.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    /// The canonicalized function path (`type::is_record`).
    pub path: Spanned<String>,
    /// The path **exactly as written**, before canonicalization — so
    /// `type::is::record` and `type::is_record` stay distinguishable. They are
    /// not interchangeable: 3.x retired every `::is::` spelling and rejects it
    /// as a parse error, and canonicalizing before anyone looks is what made
    /// the dead one indistinguishable from the live one. Shares `path`'s span;
    /// equal to `path.node` for every path canonicalization does not touch.
    pub written: String,
    /// The argument expressions, each retaining its own span.
    pub args: Vec<Spanned<Expr>>,
}

/// Binary operators, one variant per operator the engine has, plus an
/// explicit escape hatch — an unknown operator is a fact, not a guess.
///
/// The unicode spellings fold onto their keyword (`∈` is `INSIDE`, `×` is
/// `*`), and case is irrelevant (`contains` is `CONTAINS`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*` / `×`
    Mul,
    /// `/` / `÷`
    Div,
    /// `%` — remainder.
    Rem,
    /// `**` — exponentiation.
    Pow,
    /// `=` — equality.
    Eq,
    /// `==` — exact (type-strict) equality.
    Exact,
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
    /// `?:` — truthiness coalescing (the left if truthy, else the right).
    TruthyCoalesce,
    /// `IS` — equality, the keyword spelling.
    Is,
    /// `IS NOT` — inequality, the keyword spelling.
    IsNot,
    /// `IN` — the left is an element of the right collection.
    In,
    /// `NOT IN`
    NotIn,
    /// `CONTAINS` / `∋` — the left collection holds the right value.
    Contains,
    /// `CONTAINSNOT` / `∌`
    ContainsNot,
    /// `CONTAINSALL` / `⊇`
    ContainsAll,
    /// `CONTAINSANY` / `⊃`
    ContainsAny,
    /// `CONTAINSNONE` / `⊅`
    ContainsNone,
    /// `INSIDE` / `∈` — the same claim as `IN`.
    Inside,
    /// `NOTINSIDE` / `∉`
    NotInside,
    /// `ALLINSIDE` / `⊆`
    AllInside,
    /// `ANYINSIDE` / `⊂`
    AnyInside,
    /// `NONEINSIDE` / `⊄`
    NoneInside,
    /// `OUTSIDE` — geometry: the left lies entirely outside the right.
    Outside,
    /// `INTERSECTS` — geometry: the two shapes overlap.
    Intersects,
    /// `~` — fuzzy match.
    Match,
    /// `!~` — fuzzy mismatch.
    NotMatch,
    /// `*~` — every element fuzzy-matches.
    AllMatch,
    /// `?~` — some element fuzzy-matches.
    AnyMatch,
    /// `?=` — some element equals.
    AnyEq,
    /// `*=` — every element equals.
    AllEq,
    /// `@@` / `@n@` — full-text match, with the optional match reference
    /// number used by `search::highlight`/`search::score`.
    Matches(Option<i64>),
    /// `<|k|>` / `<|k, ef|>` / `<|k, DISTANCE|>` — k-nearest-neighbour
    /// vector search.
    Knn(Knn),
    /// Any operator not modeled above, kept as raw text.
    Other(String),
}

/// The parameters of a `<|…|>` KNN operator.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Knn {
    /// `k` — how many neighbours to return, when it is a literal.
    pub k: Option<i64>,
    /// The `ef` search-effort parameter of the HNSW form (`<|k, ef|>`).
    pub ef: Option<i64>,
    /// The distance metric of the brute-force form (`<|k, COSINE|>`),
    /// uppercased.
    pub distance: Option<String>,
}

/// Prefix operators, with an explicit escape hatch for the unmodeled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrefixOp {
    /// `!` / `NOT` — logical negation.
    Not,
    /// `-` — arithmetic negation.
    Neg,
    /// `+` — unary plus.
    Pos,
    /// Any operator not modeled above, kept as raw text.
    Other(String),
}
