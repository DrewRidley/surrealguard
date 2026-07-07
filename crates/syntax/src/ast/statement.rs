//! Statements.
//!
//! Statement structs model exactly what type analysis consumes. Clauses that
//! cannot affect a statement's response type (permissions, comments,
//! timeouts, index hints, ...) are consumed by lowering without record;
//! statements containing broken syntax lower to [`Statement::Partial`], so
//! analyzers never see a half-parsed structure.

use super::{
    DataClause, Expr, GroupClause, Idiom, OrderClause, PartialNode, Projection, ReturnMode,
    Spanned, TypeExpr,
};
use crate::span::ByteRange;

/// A lowered source file: statements in source order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Script {
    pub statements: Vec<Spanned<Statement>>,
}

// Variant payloads differ widely in size by design: statements are built
// once per parse and matched by reference, never stored in bulk — boxing the
// large variants would cost matching ergonomics for no real memory win.
#[allow(clippy::large_enum_variant)]
/// One SurrealQL statement, dispatched on by the analyzers.
#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    Select(SelectStmt),
    Create(CreateStmt),
    Update(UpdateStmt),
    Upsert(UpsertStmt),
    Delete(DeleteStmt),
    Insert(InsertStmt),
    Relate(RelateStmt),
    Define(DefineStmt),
    Remove(RemoveStmt),
    Alter(AlterStmt),
    Let(LetStmt),
    Return(ReturnStmt),
    IfElse(IfElseStmt),
    For(ForStmt),
    Block(Block),
    LiveSelect(LiveSelectStmt),
    Kill(KillStmt),
    Use(UseStmt),
    Info(InfoStmt),
    Show(ShowStmt),
    Rebuild(RebuildStmt),
    Throw(ThrowStmt),
    Break(BreakStmt),
    Continue(ContinueStmt),
    Begin(BeginStmt),
    Cancel(CancelStmt),
    Commit(CommitStmt),
    Sleep(SleepStmt),
    Option(OptionStmt),
    /// A bare expression in statement position — most commonly the trailing
    /// value of a block (`{ LET $x = 1; $x + 1 }`).
    Expr(Spanned<Expr>),
    Partial(PartialNode),
}

/// `{ ...; ...; }` — also the body of IF/FOR/DEFINE FUNCTION.
///
/// The block analyzer dispatches each child to that child's own analyzer;
/// per-statement invariants stay attached to their own statement kind.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    pub statements: Vec<Spanned<Statement>>,
}

/// `SELECT` — projections over one or more sources, plus its modifier
/// clauses.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectStmt {
    pub only: bool,
    /// `SELECT VALUE <expr>`.
    pub value: bool,
    pub projections: Vec<Projection>,
    pub from: Vec<Spanned<Expr>>,
    pub omit: Vec<Spanned<Idiom>>,
    pub fetch: Vec<Spanned<Idiom>>,
    pub split: Vec<Spanned<Idiom>>,
    pub where_clause: Option<Spanned<Expr>>,
    pub group: Option<GroupClause>,
    pub order: Option<OrderClause>,
    /// Literal-ness is the analyzer's judgment (`LIMIT 5` vs `LIMIT $n`).
    pub limit: Option<Spanned<Expr>>,
    pub start: Option<Spanned<Expr>>,
    pub explain: Option<ByteRange>,
    pub timeout: Option<Spanned<Expr>>,
    pub parallel: Option<ByteRange>,
}

/// `CREATE` — new rows for a table or specific record ids.
#[derive(Clone, Debug, PartialEq)]
pub struct CreateStmt {
    /// `CREATE ONLY person:one` — single object result, not an array.
    pub only: bool,
    pub targets: Vec<Spanned<Expr>>,
    pub data: Option<DataClause>,
    pub ret: Option<Spanned<ReturnMode>>,
}

/// `UPDATE` — modifies existing rows.
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateStmt {
    /// `UPDATE ONLY person:one` — single object result, not an array.
    pub only: bool,
    pub targets: Vec<Spanned<Expr>>,
    pub data: Option<DataClause>,
    pub where_clause: Option<Spanned<Expr>>,
    pub ret: Option<Spanned<ReturnMode>>,
}

/// `UPSERT` — updates rows, creating them when absent.
#[derive(Clone, Debug, PartialEq)]
pub struct UpsertStmt {
    /// `UPSERT ONLY person:one` — single object result, not an array.
    pub only: bool,
    pub targets: Vec<Spanned<Expr>>,
    pub data: Option<DataClause>,
    pub where_clause: Option<Spanned<Expr>>,
    pub ret: Option<Spanned<ReturnMode>>,
}

/// `DELETE` — removes rows.
#[derive(Clone, Debug, PartialEq)]
pub struct DeleteStmt {
    /// `DELETE ONLY person:one` — single object result, not an array.
    pub only: bool,
    pub targets: Vec<Spanned<Expr>>,
    pub where_clause: Option<Spanned<Expr>>,
    pub ret: Option<Spanned<ReturnMode>>,
}

/// `INSERT` — bulk row insertion with its own payload forms.
#[derive(Clone, Debug, PartialEq)]
pub struct InsertStmt {
    /// `INSERT IGNORE`.
    pub ignore: bool,
    /// `INSERT RELATION` (row payloads describe edges).
    pub relation: bool,
    /// `INTO <target>`.
    pub target: Option<Spanned<Expr>>,
    pub data: InsertData,
    pub ret: Option<Spanned<ReturnMode>>,
}

/// INSERT's payload forms — distinct from the other mutations' `DataClause`
/// (the grammar gives INSERT `BulkInsert`/`FieldAssignment` children the
/// others don't have).
#[derive(Clone, Debug, PartialEq)]
pub enum InsertData {
    /// Object or array-of-objects payload.
    Values(Vec<Spanned<Expr>>),
    /// `INSERT INTO t (a, b) VALUES (...), (...)` — each row is lowered to
    /// `(column, value)` pairs so columns and values cannot misalign.
    Rows(Vec<Vec<(Spanned<Idiom>, Spanned<Expr>)>>),
    /// `INSERT ... SET`-style field assignments.
    Assignments(Vec<(Spanned<Idiom>, Spanned<Expr>)>),
    Partial(PartialNode),
}

/// `RELATE from->edge->to` — three explicitly spanned positions, because
/// edge-endpoint diagnostics will point at each independently.
#[derive(Clone, Debug, PartialEq)]
pub struct RelateStmt {
    pub only: bool,
    pub from: Option<Spanned<Expr>>,
    pub edge: Option<Spanned<Expr>>,
    pub to: Option<Spanned<Expr>>,
    pub data: Option<DataClause>,
    pub ret: Option<Spanned<ReturnMode>>,
}

/// DEFINE family. Tier 1 kinds are modeled; the long tail
/// (ACCESS/API/BUCKET/CONFIG/...) is `Other` until an analyzer needs it.
#[derive(Clone, Debug, PartialEq)]
pub enum DefineStmt {
    Table(DefineTable),
    Field(DefineField),
    Index(DefineIndex),
    Event(DefineEvent),
    Param(DefineParam),
    Function(DefineFunction),
    Analyzer(DefineAnalyzer),
    Other(PartialNode),
}

/// `DEFINE TABLE`.
#[derive(Clone, Debug, PartialEq)]
pub struct DefineTable {
    pub name: Spanned<String>,
    pub overwrite: bool,
    pub schemafull: bool,
    pub relation: Option<RelationDef>,
}

/// The `IN`/`OUT` endpoint tables of a relation table.
#[derive(Clone, Debug, PartialEq)]
pub struct RelationDef {
    pub in_tables: Vec<Spanned<String>>,
    pub out_tables: Vec<Spanned<String>>,
}

/// `DEFINE FIELD` — a (possibly nested) field on a table, with its
/// declared type.
#[derive(Clone, Debug, PartialEq)]
pub struct DefineField {
    pub path: Spanned<Idiom>,
    pub table: Spanned<String>,
    pub ty: Option<Spanned<TypeExpr>>,
    pub overwrite: bool,
}

/// `DEFINE INDEX`.
#[derive(Clone, Debug, PartialEq)]
pub struct DefineIndex {
    pub name: Spanned<String>,
    pub table: Spanned<String>,
    pub fields: Vec<Spanned<Idiom>>,
    pub unique: bool,
}

/// `DEFINE EVENT` — a trigger with its condition and body.
#[derive(Clone, Debug, PartialEq)]
pub struct DefineEvent {
    pub name: Spanned<String>,
    pub table: Spanned<String>,
    pub when: Option<Spanned<Expr>>,
    /// `THEN { ... }` / `THEN <expr>` — a block lowers to `Expr::Block`.
    pub then: Option<Spanned<Expr>>,
}

/// `DEFINE PARAM` — a database-level parameter with a default value.
#[derive(Clone, Debug, PartialEq)]
pub struct DefineParam {
    pub name: Spanned<String>,
    pub value: Option<Spanned<Expr>>,
}

/// `DEFINE FUNCTION fn::name(...)`.
#[derive(Clone, Debug, PartialEq)]
pub struct DefineFunction {
    pub name: Spanned<String>,
    pub params: Vec<(Spanned<String>, Option<Spanned<TypeExpr>>)>,
    pub body: Option<Block>,
    pub return_ty: Option<Spanned<TypeExpr>>,
}

/// `DEFINE ANALYZER` — a full-text analyzer pipeline.
#[derive(Clone, Debug, PartialEq)]
pub struct DefineAnalyzer {
    pub name: Spanned<String>,
    pub tokenizers: Vec<Spanned<String>>,
    pub filters: Vec<Spanned<String>>,
}

/// `REMOVE` — drops a schema object.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoveStmt {
    pub target: RemoveTarget,
}

/// What a `REMOVE` statement drops.
#[derive(Clone, Debug, PartialEq)]
pub enum RemoveTarget {
    Table(Spanned<String>),
    Field {
        field: Spanned<Idiom>,
        table: Spanned<String>,
    },
    Index {
        index: Spanned<String>,
        table: Spanned<String>,
    },
    Other(PartialNode),
}

/// `ALTER TABLE`.
#[derive(Clone, Debug, PartialEq)]
pub struct AlterStmt {
    pub table: Option<Spanned<String>>,
}

/// `LET $name = value` — binds a statement-scope variable.
#[derive(Clone, Debug, PartialEq)]
pub struct LetStmt {
    /// Binding name without `$`.
    pub name: Spanned<String>,
    pub value: Spanned<Expr>,
}

/// `RETURN` — yields a value from the enclosing scope.
#[derive(Clone, Debug, PartialEq)]
pub struct ReturnStmt {
    pub value: Option<Spanned<Expr>>,
}

/// `IF`/`ELSE IF`/`ELSE` — conditional branches, each with a block body.
#[derive(Clone, Debug, PartialEq)]
pub struct IfElseStmt {
    /// `IF cond body` plus any `ELSE IF` arms, in source order.
    pub branches: Vec<IfBranch>,
    pub else_branch: Option<Block>,
}

/// One `IF`/`ELSE IF` arm: a condition and its body.
#[derive(Clone, Debug, PartialEq)]
pub struct IfBranch {
    pub condition: Spanned<Expr>,
    pub body: Block,
}

/// `FOR $item IN iterable { ... }`.
#[derive(Clone, Debug, PartialEq)]
pub struct ForStmt {
    /// Loop binding name without `$`.
    pub binding: Spanned<String>,
    pub iterable: Spanned<Expr>,
    pub body: Block,
}

/// `LIVE SELECT` — subscribes to changes on a table.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveSelectStmt {
    pub table: Option<Spanned<String>>,
}

/// `KILL` — terminates a live query by id.
#[derive(Clone, Debug, PartialEq)]
pub struct KillStmt {
    pub id: Option<Spanned<Expr>>,
}

/// `USE NS ... DB ...` — selects the active namespace/database.
#[derive(Clone, Debug, PartialEq)]
pub struct UseStmt {
    pub namespace: Option<Spanned<String>>,
    pub database: Option<Spanned<String>>,
}

/// `INFO FOR ...` — describes a catalog level.
#[derive(Clone, Debug, PartialEq)]
pub struct InfoStmt {
    pub table: Option<Spanned<String>>,
}

/// `SHOW CHANGES FOR TABLE ...` — reads a change feed.
#[derive(Clone, Debug, PartialEq)]
pub struct ShowStmt {
    pub table: Option<Spanned<String>>,
}

/// `REBUILD INDEX ... ON ...`.
#[derive(Clone, Debug, PartialEq)]
pub struct RebuildStmt {
    pub index: Option<Spanned<String>>,
    pub table: Option<Spanned<String>>,
}

/// `THROW` — raises an error value.
#[derive(Clone, Debug, PartialEq)]
pub struct ThrowStmt {
    pub value: Option<Spanned<Expr>>,
}

/// `SLEEP <duration>`.
#[derive(Clone, Debug, PartialEq)]
pub struct SleepStmt {
    pub duration: Option<Spanned<Expr>>,
}

macro_rules! unit_statements {
    ($($(#[$doc:meta])* $name:ident),+ $(,)?) => {
        $(
            $(#[$doc])*
            #[derive(Clone, Debug, Default, PartialEq)]
            pub struct $name {}
        )+
    };
}

unit_statements! {
    /// `BREAK`.
    BreakStmt,
    /// `CONTINUE`.
    ContinueStmt,
    /// `BEGIN [TRANSACTION]`.
    BeginStmt,
    /// `CANCEL [TRANSACTION]`.
    CancelStmt,
    /// `COMMIT [TRANSACTION]`.
    CommitStmt,
    /// `OPTION <name> [= <value>]`.
    OptionStmt,
}
