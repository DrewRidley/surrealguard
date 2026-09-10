//! Statements.
//!
//! Statement structs model exactly what type analysis consumes. Clauses that
//! cannot affect a statement's response type (comments, timeouts, index
//! hints, ...) are consumed by lowering without record; statements containing
//! broken syntax lower to [`Statement::Partial`], so analyzers never see a
//! half-parsed structure. `PERMISSIONS` predicate expressions are the
//! exception: they are analyzed (undefined fields/functions, always-false and
//! non-boolean predicates), so lowering retains each `FOR <action> WHERE
//! <expr>` predicate.

use super::{
    Assignment, DataClause, Expr, GroupClause, Idiom, OrderClause, PartialNode, Projection,
    ReturnMode, Spanned, TypeExpr,
};
use crate::span::ByteRange;

/// A lowered source file: statements in source order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Script {
    /// Top-level statements, in source order.
    pub statements: Vec<Spanned<Statement>>,
}

// Variant payloads differ widely in size by design: statements are built
// once per parse and matched by reference, never stored in bulk — boxing the
// large variants would cost matching ergonomics for no real memory win.
#[allow(clippy::large_enum_variant)]
/// One SurrealQL statement, dispatched on by the analyzers.
#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    /// `SELECT`.
    Select(SelectStmt),
    /// `CREATE`.
    Create(CreateStmt),
    /// `UPDATE`.
    Update(UpdateStmt),
    /// `UPSERT`.
    Upsert(UpsertStmt),
    /// `DELETE`.
    Delete(DeleteStmt),
    /// `INSERT`.
    Insert(InsertStmt),
    /// `RELATE`.
    Relate(RelateStmt),
    /// `DEFINE ...`.
    Define(DefineStmt),
    /// `REMOVE ...`.
    Remove(RemoveStmt),
    /// `ALTER ...`.
    Alter(AlterStmt),
    /// `LET`.
    Let(LetStmt),
    /// `RETURN`.
    Return(ReturnStmt),
    /// `IF`/`ELSE`.
    IfElse(IfElseStmt),
    /// `FOR`.
    For(ForStmt),
    /// A bare `{ ...; ... }` block in statement position.
    Block(Block),
    /// `LIVE SELECT`.
    LiveSelect(LiveSelectStmt),
    /// `KILL`.
    Kill(KillStmt),
    /// `USE`.
    Use(UseStmt),
    /// `INFO FOR ...`.
    Info(InfoStmt),
    /// `SHOW CHANGES`.
    Show(ShowStmt),
    /// `REBUILD INDEX`.
    Rebuild(RebuildStmt),
    /// `THROW`.
    Throw(ThrowStmt),
    /// `BREAK`.
    Break(BreakStmt),
    /// `CONTINUE`.
    Continue(ContinueStmt),
    /// `BEGIN`.
    Begin(BeginStmt),
    /// `CANCEL`.
    Cancel(CancelStmt),
    /// `COMMIT`.
    Commit(CommitStmt),
    /// `SLEEP`.
    Sleep(SleepStmt),
    /// `OPTION`.
    Option(OptionStmt),
    /// A bare expression in statement position — most commonly the trailing
    /// value of a block (`{ LET $x = 1; $x + 1 }`).
    Expr(Spanned<Expr>),
    /// A statement that failed to lower.
    Partial(PartialNode),
}

/// `{ ...; ...; }` — also the body of IF/FOR/DEFINE FUNCTION.
///
/// The block analyzer dispatches each child to that child's own analyzer;
/// per-statement invariants stay attached to their own statement kind.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    /// The block's statements, in source order.
    pub statements: Vec<Spanned<Statement>>,
}

/// `SELECT` — projections over one or more sources, plus its modifier
/// clauses.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectStmt {
    /// `SELECT ... FROM ONLY ...` — single-object result, not an array.
    pub only: bool,
    /// `SELECT VALUE <expr>`.
    pub value: bool,
    /// The projection list (output columns).
    pub projections: Vec<Projection>,
    /// `FROM` sources.
    pub from: Vec<Spanned<Expr>>,
    /// `OMIT <fields>` — fields excluded from the result.
    pub omit: Vec<Spanned<Idiom>>,
    /// `FETCH <fields>` — record links to expand.
    pub fetch: Vec<Spanned<Idiom>>,
    /// `SPLIT <fields>` — array fields to fan out into separate rows.
    pub split: Vec<Spanned<Idiom>>,
    /// `WHERE <expr>` filter.
    pub where_clause: Option<Spanned<Expr>>,
    /// `GROUP BY`/`GROUP ALL`.
    pub group: Option<GroupClause>,
    /// `ORDER BY`.
    pub order: Option<OrderClause>,
    /// `LIMIT <expr>`. Literal-ness is the analyzer's judgment
    /// (`LIMIT 5` vs `LIMIT $n`).
    pub limit: Option<Spanned<Expr>>,
    /// `START <expr>` — result offset.
    pub start: Option<Spanned<Expr>>,
    /// `EXPLAIN` — the clause's span, when present.
    pub explain: Option<ByteRange>,
    /// `TIMEOUT <duration>`.
    pub timeout: Option<Spanned<Expr>>,
    /// `PARALLEL` — the clause's span, when present.
    pub parallel: Option<ByteRange>,
}

/// `CREATE` — new rows for a table or specific record ids.
#[derive(Clone, Debug, PartialEq)]
pub struct CreateStmt {
    /// `CREATE ONLY person:one` — single object result, not an array.
    pub only: bool,
    /// The tables or record ids to create.
    pub targets: Vec<Spanned<Expr>>,
    /// The payload clause (`SET`/`CONTENT`/...), if any.
    pub data: Option<DataClause>,
    /// `RETURN` mode, if specified.
    pub ret: Option<Spanned<ReturnMode>>,
}

/// `UPDATE` — modifies existing rows.
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateStmt {
    /// `UPDATE ONLY person:one` — single object result, not an array.
    pub only: bool,
    /// The tables or record ids to update.
    pub targets: Vec<Spanned<Expr>>,
    /// The payload clause (`SET`/`CONTENT`/...), if any.
    pub data: Option<DataClause>,
    /// `WHERE <expr>` filter selecting which rows to update.
    pub where_clause: Option<Spanned<Expr>>,
    /// `RETURN` mode, if specified.
    pub ret: Option<Spanned<ReturnMode>>,
}

/// `UPSERT` — updates rows, creating them when absent.
#[derive(Clone, Debug, PartialEq)]
pub struct UpsertStmt {
    /// `UPSERT ONLY person:one` — single object result, not an array.
    pub only: bool,
    /// The tables or record ids to upsert.
    pub targets: Vec<Spanned<Expr>>,
    /// The payload clause (`SET`/`CONTENT`/...), if any.
    pub data: Option<DataClause>,
    /// `WHERE <expr>` filter selecting which rows to update.
    pub where_clause: Option<Spanned<Expr>>,
    /// `RETURN` mode, if specified.
    pub ret: Option<Spanned<ReturnMode>>,
}

/// `DELETE` — removes rows.
#[derive(Clone, Debug, PartialEq)]
pub struct DeleteStmt {
    /// `DELETE ONLY person:one` — single object result, not an array.
    pub only: bool,
    /// The tables or record ids to delete from.
    pub targets: Vec<Spanned<Expr>>,
    /// `WHERE <expr>` filter selecting which rows to delete.
    pub where_clause: Option<Spanned<Expr>>,
    /// `RETURN` mode, if specified.
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
    /// The rows/values to insert.
    pub data: InsertData,
    /// `ON DUPLICATE KEY UPDATE <assignments>` — the writes applied to a
    /// row that already exists, kept beside the row payload they amend.
    pub on_duplicate_update: Vec<Assignment>,
    /// `RETURN` mode, if specified.
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
    ///
    /// The grammar flattens all rows' values into one sequence, so when the
    /// total doesn't divide evenly by the column count the leftover cannot
    /// be paired: `misaligned` carries the raw `(values, columns)` counts
    /// with the statement's span for the arity finding.
    Rows {
        /// One vector of `(column, value)` pairs per input row.
        rows: Vec<Vec<(Spanned<Idiom>, Spanned<Expr>)>>,
        /// Raw `(values, columns)` counts when the flattened values don't
        /// divide evenly by the column count; carries the statement span.
        misaligned: Option<Spanned<(usize, usize)>>,
    },
    /// A payload that failed to lower.
    Partial(PartialNode),
}

/// `RELATE from->edge->to` — three explicitly spanned positions, because
/// edge-endpoint diagnostics will point at each independently.
#[derive(Clone, Debug, PartialEq)]
pub struct RelateStmt {
    /// `RELATE ONLY ...` — single-object result, not an array.
    pub only: bool,
    /// The `from` endpoint (the edge's `in`).
    pub from: Option<Spanned<Expr>>,
    /// The edge table or record being created.
    pub edge: Option<Spanned<Expr>>,
    /// The `to` endpoint (the edge's `out`).
    pub to: Option<Spanned<Expr>>,
    /// The payload clause (`SET`/`CONTENT`/...), if any.
    pub data: Option<DataClause>,
    /// `RETURN` mode, if specified.
    pub ret: Option<Spanned<ReturnMode>>,
}

/// DEFINE family. Tier 1 kinds are modeled; the long tail
/// (ACCESS/API/BUCKET/CONFIG/...) is `Other` until an analyzer needs it.
#[derive(Clone, Debug, PartialEq)]
pub enum DefineStmt {
    /// `DEFINE TABLE`.
    Table(DefineTable),
    /// `DEFINE FIELD`.
    Field(Box<DefineField>),
    /// `DEFINE INDEX`.
    Index(DefineIndex),
    /// `DEFINE EVENT`.
    Event(DefineEvent),
    /// `DEFINE PARAM`.
    Param(DefineParam),
    /// `DEFINE FUNCTION`.
    Function(DefineFunction),
    /// `DEFINE ANALYZER`.
    Analyzer(DefineAnalyzer),
    /// An unmodeled `DEFINE` kind (ACCESS/API/BUCKET/CONFIG/...).
    Other(PartialNode),
}

/// `DEFINE TABLE`.
#[derive(Clone, Debug, PartialEq)]
pub struct DefineTable {
    /// The table name.
    pub name: Spanned<String>,
    /// `OVERWRITE` — redefine an existing table.
    pub overwrite: bool,
    /// `IF NOT EXISTS` — a no-op when the table already exists.
    pub if_not_exists: bool,
    /// `SCHEMAFULL` (vs `SCHEMALESS`).
    pub schemafull: bool,
    /// `TYPE RELATION ...` — present when the table is an edge table.
    pub relation: Option<RelationDef>,
    /// `DEFINE TABLE ... DROP` — rows are never retained.
    pub drop: bool,
    /// `DEFINE TABLE ... CHANGEFEED <duration>`.
    pub changefeed: bool,
    /// `PERMISSIONS FOR <action> WHERE <expr>` predicate expressions. Each
    /// `WHERE` predicate — from either the basic (`PERMISSIONS WHERE`) or the
    /// per-action (`PERMISSIONS FOR select ... WHERE`) form — is retained so
    /// the analyzer can walk it; `NONE`/`FULL` carry no predicate.
    pub permissions: Vec<Spanned<Expr>>,
}

/// The `IN`/`OUT` endpoint tables of a relation table.
#[derive(Clone, Debug, PartialEq)]
pub struct RelationDef {
    /// Allowed `in` endpoint tables.
    pub in_tables: Vec<Spanned<String>>,
    /// Allowed `out` endpoint tables.
    pub out_tables: Vec<Spanned<String>>,
    /// The whole `TYPE RELATION ...` clause.
    pub span: ByteRange,
}

/// `DEFINE FIELD` — a (possibly nested) field on a table, with its
/// declared type.
#[derive(Clone, Debug, PartialEq)]
pub struct DefineField {
    /// The (possibly nested) field path.
    pub path: Spanned<Idiom>,
    /// The table the field belongs to.
    pub table: Spanned<String>,
    /// `TYPE <type>` — the declared field type, if any.
    pub ty: Option<Spanned<TypeExpr>>,
    /// `OVERWRITE` — redefine an existing field.
    pub overwrite: bool,
    /// `IF NOT EXISTS` — a no-op when the field already exists.
    pub if_not_exists: bool,
    /// `DEFAULT <expr>` — supplied when a row is created without the field.
    pub default: Option<Spanned<Expr>>,
    /// `VALUE <expr>` — the field is computed; writes are overwritten.
    pub value: Option<Spanned<Expr>>,
    /// `COMPUTED <expr>` — the field is derived from an expression and never
    /// stored (SurrealDB 3.0). Like `VALUE`, its type is the expression's; a
    /// common form is a record-reference back-traversal (`COMPUTED <~team`).
    pub computed: Option<Spanned<Expr>>,
    /// `REFERENCE` — the field's `record<...>` link participates in reference
    /// traversal (`<~`), so a back-reference on the target table can resolve
    /// through it.
    pub reference: bool,
    /// `ASSERT <expr>` — must hold for every write (`$value` in scope).
    pub assert: Option<Spanned<Expr>>,
    /// `READONLY` — writable only at creation.
    pub readonly: bool,
    /// `PERMISSIONS FOR <action> WHERE <expr>` predicate expressions. Each
    /// `WHERE` predicate is retained so the analyzer can walk it against the
    /// row; `NONE`/`FULL` carry no predicate.
    pub permissions: Vec<Spanned<Expr>>,
}

/// `DEFINE INDEX`.
#[derive(Clone, Debug, PartialEq)]
pub struct DefineIndex {
    /// The index name.
    pub name: Spanned<String>,
    /// `OVERWRITE` — redefine an existing index.
    pub overwrite: bool,
    /// `IF NOT EXISTS` — a no-op when the index already exists.
    pub if_not_exists: bool,
    /// The table the index is defined on.
    pub table: Spanned<String>,
    /// The indexed field paths.
    pub fields: Vec<Spanned<Idiom>>,
    /// What backs the index.
    pub kind: IndexKind,
}

/// What backs a `DEFINE INDEX`: full-text search, a vector structure, a
/// uniqueness constraint, or a plain lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexKind {
    /// A plain lookup index.
    Normal,
    /// `UNIQUE` — a uniqueness constraint.
    Unique,
    /// `SEARCH ANALYZER ...` — full-text.
    Search,
    /// `MTREE`/`HNSW` — vector.
    Vector,
}

/// `DEFINE EVENT` — a trigger with its condition and body.
#[derive(Clone, Debug, PartialEq)]
pub struct DefineEvent {
    /// The event name.
    pub name: Spanned<String>,
    /// `OVERWRITE` — redefine an existing event.
    pub overwrite: bool,
    /// `IF NOT EXISTS` — a no-op when the event already exists.
    pub if_not_exists: bool,
    /// The table the event fires on.
    pub table: Spanned<String>,
    /// `WHEN <expr>` — the trigger condition.
    pub when: Option<Spanned<Expr>>,
    /// `THEN { ... }` / `THEN <expr>` — a block lowers to `Expr::Block`.
    pub then: Option<Spanned<Expr>>,
}

/// `DEFINE PARAM` — a database-level parameter with a default value.
#[derive(Clone, Debug, PartialEq)]
pub struct DefineParam {
    /// The parameter name (without `$`).
    pub name: Spanned<String>,
    /// `OVERWRITE` — redefine an existing parameter.
    pub overwrite: bool,
    /// `IF NOT EXISTS` — a no-op when the parameter already exists.
    pub if_not_exists: bool,
    /// `VALUE <expr>` — the parameter's value.
    pub value: Option<Spanned<Expr>>,
}

/// `DEFINE FUNCTION fn::name(...)`.
#[derive(Clone, Debug, PartialEq)]
pub struct DefineFunction {
    /// The function name including the `fn::` path.
    pub name: Spanned<String>,
    /// `OVERWRITE` — redefine an existing function.
    pub overwrite: bool,
    /// `IF NOT EXISTS` — a no-op when the function already exists.
    pub if_not_exists: bool,
    /// Parameters with their declared types, if any.
    pub params: Vec<(Spanned<String>, Option<Spanned<TypeExpr>>)>,
    /// The function body.
    pub body: Option<Block>,
    /// The declared return type, if any.
    pub return_ty: Option<Spanned<TypeExpr>>,
}

/// `DEFINE ANALYZER` — a full-text analyzer pipeline.
#[derive(Clone, Debug, PartialEq)]
pub struct DefineAnalyzer {
    /// The analyzer name.
    pub name: Spanned<String>,
    /// `OVERWRITE` — redefine an existing analyzer.
    pub overwrite: bool,
    /// `IF NOT EXISTS` — a no-op when the analyzer already exists.
    pub if_not_exists: bool,
    /// `TOKENIZERS ...` — the tokenizers in the pipeline.
    pub tokenizers: Vec<Spanned<String>>,
    /// `FILTERS ...` — the token filters in the pipeline.
    pub filters: Vec<Spanned<String>>,
}

/// `REMOVE` — drops a schema object.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoveStmt {
    /// The schema object being dropped.
    pub target: RemoveTarget,
}

/// What a `REMOVE` statement drops.
#[derive(Clone, Debug, PartialEq)]
pub enum RemoveTarget {
    /// `REMOVE TABLE <name>`.
    Table(Spanned<String>),
    /// `REMOVE FIELD <field> ON <table>`.
    Field {
        /// The field path being removed.
        field: Spanned<Idiom>,
        /// The table the field is on.
        table: Spanned<String>,
    },
    /// `REMOVE INDEX <index> ON <table>`.
    Index {
        /// The index name being removed.
        index: Spanned<String>,
        /// The table the index is on.
        table: Spanned<String>,
    },
    /// `REMOVE EVENT <event> ON <table>`.
    Event {
        /// The event name being removed.
        event: Spanned<String>,
        /// The table the event is on.
        table: Spanned<String>,
    },
    /// `REMOVE FUNCTION fn::<name>` — the name includes the `fn::` path.
    Function(Spanned<String>),
    /// `REMOVE PARAM $<name>` — the name without `$`.
    Param(Spanned<String>),
    /// `REMOVE ANALYZER <name>`.
    Analyzer(Spanned<String>),
    /// An unmodeled `REMOVE` target.
    Other(PartialNode),
}

/// `ALTER TABLE`.
#[derive(Clone, Debug, PartialEq)]
pub struct AlterStmt {
    /// The table being altered.
    pub table: Option<Spanned<String>>,
    /// `SCHEMAFULL` (`Some(true)`) / `SCHEMALESS` (`Some(false)`), when the
    /// statement changes the table's schema mode.
    pub schemafull: Option<bool>,
    /// `ALTER TABLE ... DROP` — rows are no longer retained.
    pub drop: bool,
}

/// `LET $name = value` — binds a statement-scope variable.
#[derive(Clone, Debug, PartialEq)]
pub struct LetStmt {
    /// Binding name without `$`.
    pub name: Spanned<String>,
    /// The bound value expression.
    pub value: Spanned<Expr>,
}

/// `RETURN` — yields a value from the enclosing scope.
#[derive(Clone, Debug, PartialEq)]
pub struct ReturnStmt {
    /// The returned value expression, if any.
    pub value: Option<Spanned<Expr>>,
}

/// `IF`/`ELSE IF`/`ELSE` — conditional branches, each with a block body.
#[derive(Clone, Debug, PartialEq)]
pub struct IfElseStmt {
    /// `IF cond body` plus any `ELSE IF` arms, in source order.
    pub branches: Vec<IfBranch>,
    /// The trailing `ELSE` block, if any.
    pub else_branch: Option<Block>,
}

/// One `IF`/`ELSE IF` arm: a condition and its body.
#[derive(Clone, Debug, PartialEq)]
pub struct IfBranch {
    /// The branch condition.
    pub condition: Spanned<Expr>,
    /// The block run when the condition holds.
    pub body: Block,
}

/// `FOR $item IN iterable { ... }`.
#[derive(Clone, Debug, PartialEq)]
pub struct ForStmt {
    /// Loop binding name without `$`.
    pub binding: Spanned<String>,
    /// The expression iterated over.
    pub iterable: Spanned<Expr>,
    /// The loop body.
    pub body: Block,
}

/// `LIVE SELECT` — subscribes to changes on a table.
///
/// The projection, `WHERE` and `FETCH` clauses are the same positions a
/// `SELECT` has and are checked by the same analysis; the statement's own
/// response is the subscription id.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveSelectStmt {
    /// `LIVE SELECT DIFF` — notifications carry JSON patches, not rows.
    pub diff: bool,
    /// `LIVE SELECT VALUE <expr>`.
    pub value: bool,
    /// The projection list (empty for `DIFF`).
    pub projections: Vec<Projection>,
    /// `FROM` sources — a table, a record id or a parameter.
    pub from: Vec<Spanned<Expr>>,
    /// `WHERE <expr>` filter.
    pub where_clause: Option<Spanned<Expr>>,
    /// `FETCH <fields>` — record links to expand in each notification.
    pub fetch: Vec<Spanned<Idiom>>,
}

impl LiveSelectStmt {
    /// The table the subscription reads, when the source names one directly.
    pub fn table(&self) -> Option<&Spanned<String>> {
        match self.from.first().map(|source| &source.node) {
            Some(Expr::Table(name)) => Some(name),
            _ => None,
        }
    }
}

/// `KILL` — terminates a live query by id.
#[derive(Clone, Debug, PartialEq)]
pub struct KillStmt {
    /// The live-query id to terminate.
    pub id: Option<Spanned<Expr>>,
}

/// `USE NS ... DB ...` — selects the active namespace/database.
#[derive(Clone, Debug, PartialEq)]
pub struct UseStmt {
    /// `NS <name>` — the namespace to switch to.
    pub namespace: Option<Spanned<String>>,
    /// `DB <name>` — the database to switch to.
    pub database: Option<Spanned<String>>,
}

/// `INFO FOR ...` — describes a catalog level.
#[derive(Clone, Debug, PartialEq)]
pub struct InfoStmt {
    /// `INFO FOR TABLE <name>` — the table, when scoped to one.
    pub table: Option<Spanned<String>>,
}

/// `SHOW CHANGES FOR TABLE ...` — reads a change feed.
#[derive(Clone, Debug, PartialEq)]
pub struct ShowStmt {
    /// The table whose change feed is read.
    pub table: Option<Spanned<String>>,
    /// `SINCE <versionstamp|datetime>` — the raw expression.
    pub since: Option<Spanned<Expr>>,
}

/// `REBUILD INDEX ... ON ...`.
#[derive(Clone, Debug, PartialEq)]
pub struct RebuildStmt {
    /// The index being rebuilt.
    pub index: Option<Spanned<String>>,
    /// The table the index is on.
    pub table: Option<Spanned<String>>,
}

/// `THROW` — raises an error value.
#[derive(Clone, Debug, PartialEq)]
pub struct ThrowStmt {
    /// The error value expression.
    pub value: Option<Spanned<Expr>>,
}

/// `SLEEP <duration>`.
#[derive(Clone, Debug, PartialEq)]
pub struct SleepStmt {
    /// The sleep duration expression.
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
