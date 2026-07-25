//! Statement lowering.
//!
//!
//! CST shapes encoded here (verified via `examples/dump_cst.rs`):
//! - `FROM` sources are bare value nodes after the `FROM` keyword
//!   (`Ident`, `RecordId[RecordTbIdent, RecordIdIdent]`, `VariableName`,
//!   `SubQuery`, `Path` for graph sources); `ONLY` is a keyword between
//!   `FROM` and the source.
//! - `VALUE` is a keyword inside the `Fields` node.
//! - A projection is `Predicate[expr, Keyword AS, Ident]`.
//! - `OMIT` wraps its paths in `Predicate`s; `FETCH`/`SPLIT`/`GROUP` wrap
//!   theirs in `Idiom`s.
//! - `ORDER BY` holds `Order[Idiom, Keyword DESC?]` children; `GROUP ALL`
//!   is a `GroupClause` with keywords only.
//! - `LIMIT n START m` arrives as a `LimitStartComboClause` — flattened
//!   here into the separate fields.

use tree_sitter::Node;

use super::expr::{lower_expr, lower_idiom_node};
use super::{node_range, partial};
use crate::ast::{
    AlterStmt, AssignOp, Assignment, BeginStmt, BreakStmt, CancelStmt, CommitStmt, ContinueStmt,
    CreateStmt, DataClause, DefineAnalyzer, DefineEvent, DefineField, DefineFunction, DefineIndex,
    DefineParam, DefineStmt, DefineTable, DeleteStmt, ForStmt, GroupClause, Idiom, IfBranch,
    IfElseStmt, IndexKind, InfoStmt, InsertData, InsertStmt, KillStmt, LetStmt, LiveSelectStmt,
    OptionStmt, OrderClause, OrderKey, Projection, RebuildStmt, RelateStmt, RelationDef,
    RemoveStmt, RemoveTarget, ReturnMode, ReturnStmt, SelectStmt, ShowStmt, SleepStmt, Spanned,
    Statement, ThrowStmt, UpdateStmt, UpsertStmt, UseStmt,
};
use crate::ast::{Expr, Literal};

/// Lowers one statement-position CST node.
///
/// A statement containing broken syntax anywhere in its subtree (`ERROR`
/// or `MISSING` nodes — including zero-width recovered identifiers) lowers
/// to [`Statement::Partial`]: analyzers never see a half-parsed structure,
/// and the parse-level diagnostics already report the breakage. Clauses
/// that cannot affect the statement's response type are consumed without
/// record.
/// Lowers every top-level statement of a parsed source, in source order.
/// This is the one place consumers get statements from — they never walk
/// the CST themselves.
pub fn lower_statements(parsed: &crate::parse::ParsedSource) -> Vec<Spanned<Statement>> {
    let root = parsed.tree().root_node();
    let mut out = Vec::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if !child.is_named() || matches!(child.kind(), "Comment" | "BlockComment") {
            continue;
        }
        recover_statement(child, parsed.text(), &mut out);
    }
    out
}

/// Whether a CST node sits in statement position (a `…Statement` or a `Block`).
/// Used only when *salvaging* from a broken subtree — bare expressions in
/// statement position (a block's trailing value) are lowered directly by
/// [`recover_statement`], not salvaged.
fn is_statement_node(node: Node<'_>) -> bool {
    node.kind().ends_with("Statement") || node.kind() == "Block"
}

/// Lowers one container child (a top-level or block statement position),
/// recovering the valid statements around a broken sibling.
///
/// tree-sitter's error recovery frequently *nests* the statement that follows
/// a syntax error inside the broken statement's own subtree (e.g. a trailing
/// `LET` absorbed after an `ERROR` token). A naive per-child lowering would
/// lose every such following statement, darkening the whole block for editor
/// features. Instead: a clean child (or a recoverable container, which
/// localizes its own inner breakage) lowers whole via [`lower_statement`]
/// (which also handles a bare trailing expression); a broken non-container
/// statement lowers to a single `Partial` placeholder and its subtree is
/// scanned for nested clean statements to salvage.
pub(crate) fn recover_statement(node: Node<'_>, text: &str, out: &mut Vec<Spanned<Statement>>) {
    if !node.has_error() || is_recoverable_container(node) {
        out.push(lower_statement(node, text));
        return;
    }
    // Broken non-container statement: one honest `Partial` for the broken
    // region, then salvage any clean statements tree-sitter nested inside.
    out.push(Spanned::new(Statement::Partial(partial(node)), node_range(node)));
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        salvage_statements(child, text, out);
    }
}

/// Pulls clean statements (and recoverable containers) out of a broken
/// region, without emitting further `Partial` placeholders for the broken
/// wrappers along the way. Only genuine statement-position nodes are
/// salvaged — a stray sub-expression inside the broken statement is left to
/// its `Partial`.
fn salvage_statements(node: Node<'_>, text: &str, out: &mut Vec<Spanned<Statement>>) {
    if is_statement_node(node) && (!node.has_error() || is_recoverable_container(node)) {
        out.push(lower_statement(node, text));
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        salvage_statements(child, text, out);
    }
}

/// Lowers one statement-position CST node to a [`Statement`].
///
/// A node containing broken syntax anywhere in its subtree lowers to
/// [`Statement::Partial`] so analyzers never see a half-parsed structure.
/// Internal to the crate; consumers reach statements through
/// [`lower_statements`] or [`crate::lower::lower_first_statement`].
pub(crate) fn lower_statement(node: Node<'_>, text: &str) -> Spanned<Statement> {
    // A broken subtree normally collapses the whole statement to `Partial`, so
    // analyzers never type a half-parsed clause. But a *container* statement —
    // one whose body is an independent statement list (a `DEFINE FUNCTION`
    // body, a `{ }` block, a `FOR` body, an `IF/ELSE` branch) — can localize
    // the breakage: its broken child lowers to `Partial` while its well-formed
    // siblings lower and type normally. Descending into those keeps a single
    // mistyped statement from darkening the entire function/block for editor
    // features, without ever showing a guessed type (the broken child still
    // contributes nothing). Every other statement folds its clauses into a
    // response type, so an internal error there must still collapse it.
    if node.has_error() && !is_recoverable_container(node) {
        return Spanned::new(Statement::Partial(partial(node)), node_range(node));
    }

    let statement = match node.kind() {
        "SelectStatement" => Statement::Select(lower_select(node, text)),
        "CreateStatement" => Statement::Create(lower_create(node, text)),
        "UpdateStatement" => Statement::Update(lower_update(node, text)),
        "UpsertStatement" => Statement::Upsert(lower_upsert(node, text)),
        "DeleteStatement" => Statement::Delete(lower_delete(node, text)),
        "InsertStatement" => Statement::Insert(lower_insert(node, text)),
        "RelateStatement" => Statement::Relate(lower_relate(node, text)),
        "LetStatement" => Statement::Let(lower_let(node, text)),
        "ReturnStatement" => Statement::Return(lower_return(node, text)),
        "IfElseStatement" => Statement::IfElse(lower_if_else(node, text)),
        "ForStatement" => Statement::For(lower_for(node, text)),
        "Block" => Statement::Block(super::expr::lower_block_node(node, text)),
        "ThrowStatement" => Statement::Throw(ThrowStmt {
            value: statement_value_expr(node, text),
        }),
        "BreakStatement" => Statement::Break(BreakStmt::default()),
        "ContinueStatement" => Statement::Continue(ContinueStmt::default()),
        "BeginStatement" => Statement::Begin(BeginStmt::default()),
        "CancelStatement" => Statement::Cancel(CancelStmt::default()),
        "CommitStatement" => Statement::Commit(CommitStmt::default()),
        "OptionStatement" => Statement::Option(OptionStmt::default()),
        "SleepStatement" => Statement::Sleep(SleepStmt {
            duration: statement_value_expr(node, text),
        }),
        "KillStatement" => Statement::Kill(KillStmt {
            id: statement_value_expr(node, text),
        }),
        "UseStatement" => Statement::Use(lower_use(node, text)),
        "LiveSelectStatement" => Statement::LiveSelect(LiveSelectStmt {
            table: table_after_keyword(node, text, "from"),
        }),
        "InfoForStatement" => Statement::Info(InfoStmt {
            // `INFO FOR TABLE x` and its `TB` short form.
            table: table_after_keyword(node, text, "table")
                .or_else(|| table_after_keyword(node, text, "tb")),
        }),
        "ShowStatement" => Statement::Show(lower_show(node, text)),
        "RebuildStatement" => Statement::Rebuild(lower_rebuild(node, text)),
        "DefineStatement" => Statement::Define(lower_define(node, text)),
        // A bare expression in statement position (e.g. a block's trailing
        // value).
        "Number" | "String" | "Bool" | "None" | "Duration" | "Regex" | "VariableName"
        | "RecordId" | "Array" | "Object" | "BinaryExpression" | "PrefixExpression"
        | "FunctionCall" | "TypeCast" | "SubQuery" | "Path" | "Idiom" | "Ident" => {
            Statement::Expr(lower_expr(node, text))
        }
        "RemoveStatement" => Statement::Remove(lower_remove(node, text)),
        "AlterStatement" => Statement::Alter(AlterStmt {
            table: table_after_keyword(node, text, "table"),
        }),
        _ => Statement::Partial(partial(node)),
    };
    Spanned::new(statement, node_range(node))
}

/// Whether a statement's breakage can be localized to an inner statement
/// rather than collapsing the whole statement to `Partial`. True for the
/// statement kinds whose body is an independent statement list — a `{ }`
/// block, a `FOR` body, an `IF/ELSE` branch, and a `DEFINE FUNCTION` body
/// (the only `DEFINE` that carries a `Block`). For these, lowering descends
/// and each broken child statement lowers to `Partial` on its own, so a valid
/// sibling `LET`/expression still lowers and types. Every other statement
/// (SELECT/CREATE/… and the non-function DEFINEs) folds a parse error into its
/// response type or schema effect, so it stays collapsed for soundness.
fn is_recoverable_container(node: Node<'_>) -> bool {
    match node.kind() {
        "Block" | "ForStatement" | "IfElseStatement" => true,
        "DefineStatement" => {
            let mut cursor = node.walk();
            let has_block = node.children(&mut cursor).any(|child| child.kind() == "Block");
            has_block
        }
        _ => false,
    }
}

fn lower_select(node: Node<'_>, text: &str) -> SelectStmt {
    let mut stmt = SelectStmt {
        only: false,
        value: false,
        projections: Vec::new(),
        from: Vec::new(),
        omit: Vec::new(),
        fetch: Vec::new(),
        split: Vec::new(),
        where_clause: None,
        group: None,
        order: None,
        limit: None,
        start: None,
        explain: None,
        timeout: None,
        parallel: None,
    };
    let mut saw_from = false;
    let mut saw_projections = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if child.is_error() || child.is_missing() {
            continue;
        }
        match child.kind() {
            "Keyword" => {
                let keyword = &text[child.byte_range()];
                if keyword.eq_ignore_ascii_case("from") {
                    saw_from = true;
                } else if saw_from && keyword.eq_ignore_ascii_case("only") {
                    stmt.only = true;
                }
            }
            // The first Fields node is the projection list; a later one
            // belongs to a clause this statement does not take (e.g. RETURN).
            "Fields" if !saw_projections => {
                saw_projections = true;
                let (projections, value) = lower_fields(child, text);
                stmt.projections = projections;
                stmt.value = value;
            }
            "OmitClause" => stmt.omit = clause_idioms(child, text),
            "FetchClause" => stmt.fetch = clause_idioms(child, text),
            "SplitClause" => stmt.split = clause_idioms(child, text),
            "WhereClause" => stmt.where_clause = clause_expr(child, text),
            "GroupClause" => {
                let keys = clause_idioms(child, text);
                stmt.group = Some(GroupClause {
                    all: keys.is_empty(),
                    keys,
                });
            }
            "OrderClause" => stmt.order = Some(lower_order(child, text)),
            "LimitClause" => stmt.limit = clause_expr(child, text),
            "StartClause" => stmt.start = clause_expr(child, text),
            "LimitStartComboClause" => {
                for clause in named_children(child) {
                    match clause.kind() {
                        "LimitClause" => stmt.limit = clause_expr(clause, text),
                        "StartClause" => stmt.start = clause_expr(clause, text),
                        _ => {}
                    }
                }
            }
            "TimeoutClause" => stmt.timeout = clause_expr(child, text),
            "ParallelClause" => stmt.parallel = Some(node_range(child)),
            "ExplainClause" => stmt.explain = Some(node_range(child)),
            _ if saw_from && is_source_node(child) => {
                stmt.from.push(lower_source(child, text));
            }
            // Everything unconsumed is an explicit fact, never dropped:
            // ReturnClause, WithClause, VersionClause, TempfilesClause, ...
            _ => {}
        }
    }

    stmt
}

/// Lowers a `Fields` node to projections plus the `VALUE` flag. Shared by
/// SELECT projection lists and mutation `RETURN <fields>` clauses.
fn lower_fields(fields: Node<'_>, text: &str) -> (Vec<Projection>, bool) {
    let mut projections = Vec::new();
    let mut value = false;
    for child in named_children(fields) {
        match child.kind() {
            "Keyword" if text[child.byte_range()].eq_ignore_ascii_case("value") => {
                value = true;
            }
            "Keyword" => {}
            "Any" => projections.push(Projection::Wildcard(node_range(child))),
            "Predicate" => projections.push(lower_projection(child, text)),
            _ => projections.push(Projection::Partial(partial(child))),
        }
    }
    (projections, value)
}

fn lower_projection(predicate: Node<'_>, text: &str) -> Projection {
    let mut expr_node = None;
    let mut alias = None;
    let mut saw_as = false;

    for child in named_children(predicate) {
        if child.kind() == "Keyword" {
            if text[child.byte_range()].eq_ignore_ascii_case("as") {
                saw_as = true;
            }
            continue;
        }
        if saw_as && child.kind() == "Ident" && alias.is_none() {
            alias = Some(Spanned::new(
                text[child.byte_range()].to_string(),
                node_range(child),
            ));
            continue;
        }
        if expr_node.is_none() {
            expr_node = Some(child);
        }
    }

    match expr_node {
        Some(expr_node) => Projection::Expr {
            expr: lower_expr(expr_node, text),
            alias,
        },
        None => Projection::Partial(partial(predicate)),
    }
}

/// Lowers a source/target-position node. A bare identifier here names a
/// table (`FROM person`) — unlike expression position, where it would be a
/// field path — so it lowers to [`Expr::Table`]; everything else lowers as
/// an ordinary expression.
fn lower_source(node: Node<'_>, text: &str) -> Spanned<Expr> {
    let span = node_range(node);
    match node.kind() {
        "Ident" => Spanned::new(
            Expr::Table(Spanned::new(text[node.byte_range()].to_string(), span)),
            span,
        ),
        _ => lower_expr(node, text),
    }
}

fn is_source_node(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "Ident" | "RecordId" | "VariableName" | "SubQuery" | "Path" | "Thing" | "Identifier"
    )
}

/// Paths listed in a clause, whether wrapped in `Predicate` (OMIT) or
/// `Idiom` (FETCH/SPLIT/GROUP).
fn clause_idioms(clause: Node<'_>, text: &str) -> Vec<Spanned<Idiom>> {
    let mut idioms = Vec::new();
    collect_clause_idioms(clause, text, &mut idioms);
    idioms
}

fn collect_clause_idioms(node: Node<'_>, text: &str, out: &mut Vec<Spanned<Idiom>>) {
    for child in named_children(node) {
        match child.kind() {
            "Ident" | "Path" | "Idiom" => {
                out.push(Spanned::new(
                    lower_idiom_node(child, text),
                    node_range(child),
                ));
            }
            "Predicate" => collect_clause_idioms(child, text, out),
            _ => {}
        }
    }
}

/// Collects the predicate expressions from a `PERMISSIONS` clause. `NONE` and
/// `FULL` carry no predicate; the basic form contributes its `WHERE <expr>`,
/// and the per-action form contributes each `FOR <action> WHERE <expr>`
/// group's predicate. Every predicate is a plain condition SurrealDB evaluates
/// against the row (`core/src/doc/check.rs`), so the analyzer walks them like
/// any `WHERE` clause.
fn lower_permission_predicates(
    clause: Node<'_>,
    text: &str,
    out: &mut Vec<Spanned<crate::ast::Expr>>,
) {
    for child in named_children(clause) {
        match child.kind() {
            "WhereClause" => {
                if let Some(expr) = clause_expr(child, text) {
                    out.push(expr);
                }
            }
            "PermissionGroup" => {
                for group_child in named_children(child) {
                    if group_child.kind() == "WhereClause" {
                        if let Some(expr) = clause_expr(group_child, text) {
                            out.push(expr);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// The single expression of a clause like `WHERE <e>` / `LIMIT <e>`.
fn clause_expr(clause: Node<'_>, text: &str) -> Option<Spanned<crate::ast::Expr>> {
    named_children(clause)
        .into_iter()
        .rfind(|child| child.kind() != "Keyword")
        .map(|node| lower_expr(node, text))
}

fn lower_order(clause: Node<'_>, text: &str) -> OrderClause {
    let mut keys = Vec::new();
    for order in named_children(clause) {
        if order.kind() != "Order" {
            continue;
        }
        let Some(idiom) = named_children(order)
            .into_iter()
            .find(|c| matches!(c.kind(), "Ident" | "Path" | "Idiom"))
        else {
            continue;
        };
        let descending = named_children(order)
            .into_iter()
            .any(|c| c.kind() == "Keyword" && text[c.byte_range()].eq_ignore_ascii_case("desc"));
        keys.push(OrderKey {
            expr: lower_expr(idiom, text),
            descending,
        });
    }
    OrderClause { keys }
}

fn named_children<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut cursor = node.walk();
    let children = node
        .children(&mut cursor)
        .filter(tree_sitter::Node::is_named)
        .collect();
    children
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

/// The clause set shared by CREATE/UPDATE/UPSERT/DELETE (and mostly RELATE):
/// collected in one walk, then each statement keeps the fields it models and
/// buckets the rest.
#[derive(Default)]
struct MutationParts {
    only: bool,
    targets: Vec<Spanned<Expr>>,
    data: Option<DataClause>,
    where_clause: Option<Spanned<crate::ast::Expr>>,
    ret: Option<Spanned<ReturnMode>>,
}

fn mutation_parts(node: Node<'_>, text: &str) -> MutationParts {
    let mut parts = MutationParts::default();
    let mut saw_statement_keyword = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if child.is_error() || child.is_missing() {
            continue;
        }
        match child.kind() {
            "Keyword" => {
                let keyword = &text[child.byte_range()];
                if !saw_statement_keyword {
                    saw_statement_keyword = true;
                } else if keyword.eq_ignore_ascii_case("only") {
                    parts.only = true;
                }
            }
            "SetClause" => parts.data = Some(lower_set_clause(child, text)),
            "UnsetClause" => {
                parts.data = Some(DataClause::Unset(clause_idioms(child, text)));
            }
            "ContentClause" => parts.data = data_value_clause(child, text, DataClause::Content),
            "MergeClause" => parts.data = data_value_clause(child, text, DataClause::Merge),
            "PatchClause" => parts.data = data_value_clause(child, text, DataClause::Patch),
            "ReplaceClause" => parts.data = data_value_clause(child, text, DataClause::Replace),
            "WhereClause" => parts.where_clause = clause_expr(child, text),
            "ReturnClause" => parts.ret = Some(lower_return_mode(child, text)),
            _ if is_source_node(child) => parts.targets.push(lower_source(child, text)),
            _ => {}
        }
    }

    parts
}

fn lower_create(node: Node<'_>, text: &str) -> CreateStmt {
    let parts = mutation_parts(node, text);
    // CREATE takes no WHERE clause; one produced by the permissive grammar
    // cannot affect the response type and is left for validation to flag.
    CreateStmt {
        only: parts.only,
        targets: parts.targets,
        data: parts.data,
        ret: parts.ret,
    }
}

fn lower_update(node: Node<'_>, text: &str) -> UpdateStmt {
    let parts = mutation_parts(node, text);
    UpdateStmt {
        only: parts.only,
        targets: parts.targets,
        data: parts.data,
        where_clause: parts.where_clause,
        ret: parts.ret,
    }
}

fn lower_upsert(node: Node<'_>, text: &str) -> UpsertStmt {
    let parts = mutation_parts(node, text);
    UpsertStmt {
        only: parts.only,
        targets: parts.targets,
        data: parts.data,
        where_clause: parts.where_clause,
        ret: parts.ret,
    }
}

fn lower_delete(node: Node<'_>, text: &str) -> DeleteStmt {
    let parts = mutation_parts(node, text);
    // DELETE takes no payload clause; one produced by the permissive grammar
    // cannot affect the response type and is left for validation to flag.
    DeleteStmt {
        only: parts.only,
        targets: parts.targets,
        where_clause: parts.where_clause,
        ret: parts.ret,
    }
}

fn lower_insert(node: Node<'_>, text: &str) -> InsertStmt {
    let mut stmt = InsertStmt {
        ignore: false,
        relation: false,
        target: None,
        data: InsertData::Values(Vec::new()),
        ret: None,
    };
    let mut saw_into = false;
    let mut saw_values = false;
    let mut columns: Vec<Spanned<Idiom>> = Vec::new();
    let mut values: Vec<Spanned<Expr>> = Vec::new();
    let mut assignments: Vec<(Spanned<Idiom>, Spanned<Expr>)> = Vec::new();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if child.is_error() || child.is_missing() {
            continue;
        }
        match child.kind() {
            "Keyword" => {
                let keyword = &text[child.byte_range()];
                if keyword.eq_ignore_ascii_case("into") {
                    saw_into = true;
                } else if keyword.eq_ignore_ascii_case("ignore") {
                    stmt.ignore = true;
                } else if keyword.eq_ignore_ascii_case("relation") {
                    stmt.relation = true;
                } else if keyword.eq_ignore_ascii_case("values") {
                    saw_values = true;
                }
            }
            "ReturnClause" => stmt.ret = Some(lower_return_mode(child, text)),
            "FieldAssignment" => {
                if let Some(assignment) = lower_assignment(child, text) {
                    assignments.push((assignment.target, assignment.value));
                }
            }
            // The target is the first source after INTO; later bare idents
            // (before VALUES) are tuple-insert column names.
            "Ident" if saw_into && stmt.target.is_some() && !saw_values => {
                columns.push(Spanned::new(
                    super::expr::lower_idiom_node(child, text),
                    node_range(child),
                ));
            }
            _ if saw_into && stmt.target.is_none() && is_source_node(child) => {
                stmt.target = Some(lower_source(child, text));
            }
            _ if saw_values => values.push(lower_expr(child, text)),
            // `INSERT INTO t [{…}, {…}]` — the grammar gives the bracketed
            // payload its own `BulkInsert` node (`'[' csep(Object) ']'`)
            // rather than an `Array`, so it must be lowered explicitly.
            // It becomes an ordinary `Expr::Array` of the row objects: the
            // payload keeps its source shape, and every payload check reads
            // one array of rows instead of a special case.
            "BulkInsert" => values.push(Spanned::new(
                Expr::Array(
                    named_children(child)
                        .into_iter()
                        .map(|row| lower_expr(row, text))
                        .collect(),
                ),
                node_range(child),
            )),
            "Object" | "Array" => values.push(lower_expr(child, text)),
            _ => {}
        }
    }

    stmt.data = if !assignments.is_empty() {
        InsertData::Assignments(assignments)
    } else if saw_values && !columns.is_empty() {
        // The grammar flattens `(a, b) VALUES (1, 2), (3, 4)` — rows are
        // rebuilt by chunking on the column count, pairing each value with
        // its column so misalignment is impossible. A total that doesn't
        // divide evenly is recorded for the arity finding; the partial
        // trailing chunk is dropped.
        let misaligned = (!values.len().is_multiple_of(columns.len()))
            .then(|| Spanned::new((values.len(), columns.len()), node_range(node)));
        let rows = values
            .chunks(columns.len())
            .filter(|chunk| chunk.len() == columns.len())
            .map(|chunk| columns.iter().cloned().zip(chunk.iter().cloned()).collect())
            .collect();
        InsertData::Rows { rows, misaligned }
    } else {
        InsertData::Values(values)
    };

    stmt
}

fn lower_relate(node: Node<'_>, text: &str) -> RelateStmt {
    let mut stmt = RelateStmt {
        only: false,
        from: None,
        edge: None,
        to: None,
        data: None,
        ret: None,
    };
    let mut lookups_seen = 0usize;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if child.is_error() || child.is_missing() {
            continue;
        }
        match child.kind() {
            "Keyword" if text[child.byte_range()].eq_ignore_ascii_case("only") => {
                stmt.only = true;
            }
            "Keyword" => {}
            "LookupRight" | "LookupLeft" | "LookupBoth" => lookups_seen += 1,
            "SetClause" => stmt.data = Some(lower_set_clause(child, text)),
            "ContentClause" => stmt.data = data_value_clause(child, text, DataClause::Content),
            "ReturnClause" => stmt.ret = Some(lower_return_mode(child, text)),
            _ if is_source_node(child) => {
                let source = lower_source(child, text);
                match lookups_seen {
                    0 => stmt.from = Some(source),
                    1 => stmt.edge = Some(source),
                    2 => stmt.to = Some(source),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    stmt
}

fn lower_set_clause(clause: Node<'_>, text: &str) -> DataClause {
    let assignments = named_children(clause)
        .into_iter()
        .filter(|child| child.kind() == "FieldAssignment")
        .filter_map(|assignment| lower_assignment(assignment, text))
        .collect();
    DataClause::Set(assignments)
}

fn lower_assignment(node: Node<'_>, text: &str) -> Option<Assignment> {
    let children = named_children(node);
    let op_index = children.iter().position(|c| c.kind() == "Operator")?;
    let target = children[..op_index]
        .iter()
        .find(|c| matches!(c.kind(), "Ident" | "Path" | "Idiom"))?;
    let value = children.get(op_index + 1)?;
    let op_node = children[op_index];
    let op = match &text[op_node.byte_range()] {
        "=" => AssignOp::Assign,
        "+=" => AssignOp::Add,
        "-=" => AssignOp::Sub,
        "+?=" => AssignOp::Extend,
        other => AssignOp::Other(other.to_string()),
    };

    Some(Assignment {
        target: Spanned::new(
            super::expr::lower_idiom_node(*target, text),
            node_range(*target),
        ),
        op: Spanned::new(op, node_range(op_node)),
        value: lower_expr(*value, text),
    })
}

fn data_value_clause(
    clause: Node<'_>,
    text: &str,
    build: impl FnOnce(Spanned<Expr>) -> DataClause,
) -> Option<DataClause> {
    clause_expr(clause, text).map(build)
}

/// Parses `RETURN NONE|NULL|DIFF|BEFORE|AFTER|<fields>` structurally:
/// `RETURN nonexistent_field` is a fields return, never `NONE`, no matter
/// what keyword substrings the field name contains.
fn lower_return_mode(clause: Node<'_>, text: &str) -> Spanned<ReturnMode> {
    let span = node_range(clause);
    for child in named_children(clause) {
        match child.kind() {
            // BEFORE / AFTER / DIFF arrive as a Literal token.
            "Literal" => {
                let mode = match text[child.byte_range()].to_ascii_uppercase().as_str() {
                    "AFTER" => ReturnMode::After,
                    "BEFORE" => ReturnMode::Before,
                    "DIFF" => ReturnMode::Diff,
                    "NONE" => ReturnMode::None,
                    "NULL" => ReturnMode::Null,
                    _ => ReturnMode::Fields(vec![Projection::Partial(partial(child))]),
                };
                return Spanned::new(mode, span);
            }
            // NONE / NULL / field lists arrive as a Fields node.
            "Fields" => {
                let (projections, _) = lower_fields(child, text);
                if let [Projection::Expr { expr, alias: None }] = projections.as_slice() {
                    match &expr.node {
                        Expr::Literal(Literal::None) => {
                            return Spanned::new(ReturnMode::None, span);
                        }
                        Expr::Literal(Literal::Null) => {
                            return Spanned::new(ReturnMode::Null, span);
                        }
                        _ => {}
                    }
                }
                return Spanned::new(ReturnMode::Fields(projections), span);
            }
            _ => {}
        }
    }
    Spanned::new(
        ReturnMode::Fields(vec![Projection::Partial(partial(clause))]),
        span,
    )
}

// ---------------------------------------------------------------------------
// Flow and thin statements
// ---------------------------------------------------------------------------

fn lower_let(node: Node<'_>, text: &str) -> LetStmt {
    let mut name = None;
    let mut value = None;

    for child in named_children(node) {
        match child.kind() {
            "Keyword" => {}
            "ParamDefinition" => {
                if let Some(variable) = named_children(child)
                    .into_iter()
                    .find(|c| c.kind() == "VariableName")
                {
                    name = Some(Spanned::new(
                        text[variable.byte_range()]
                            .trim_start_matches('$')
                            .to_string(),
                        node_range(variable),
                    ));
                }
            }
            _ if child.is_error() || child.is_missing() => {}
            _ if value.is_none() => value = Some(lower_expr(child, text)),
            _ => {}
        }
    }

    let span = node_range(node);
    LetStmt {
        name: name.unwrap_or_else(|| Spanned::new(String::new(), span)),
        value: value.unwrap_or_else(|| Spanned::new(Expr::Partial(partial(node)), span)),
    }
}

fn lower_return(node: Node<'_>, text: &str) -> ReturnStmt {
    ReturnStmt {
        value: statement_value_expr(node, text),
    }
}

/// `IF cond { .. } ELSE IF cond { .. } ELSE { .. }` — the grammar wraps the
/// modern arms in a `Modern` node as an alternating condition/block sequence.
/// The deprecated `IF cond THEN body ELSE IF cond THEN body ELSE body END`
/// form arrives as a `Legacy` node whose bodies may be bare statements or
/// values rather than blocks; both forms lower to the same branch/else shape
/// so flow analysis (divergence, narrowing) treats them identically.
fn lower_if_else(node: Node<'_>, text: &str) -> IfElseStmt {
    let mut stmt = IfElseStmt {
        branches: Vec::new(),
        else_branch: None,
    };

    for child in named_children(node) {
        match child.kind() {
            "Keyword" => {}
            "Modern" => {
                let mut pending_condition = None;
                for arm in named_children(child) {
                    match arm.kind() {
                        "Keyword" => {}
                        "Block" => {
                            let body = super::expr::lower_block_node(arm, text);
                            match pending_condition.take() {
                                Some(condition) => stmt.branches.push(IfBranch { condition, body }),
                                None => stmt.else_branch = Some(body),
                            }
                        }
                        _ if arm.is_error() || arm.is_missing() => {}
                        _ => pending_condition = Some(lower_expr(arm, text)),
                    }
                }
            }
            "Legacy" => lower_legacy_if(child, text, &mut stmt),
            _ => {}
        }
    }

    stmt
}

/// The slot the next non-keyword `Legacy` child fills, tracked through the
/// THEN/ELSE/IF/END keyword sequence.
enum LegacySlot {
    Condition,
    Body,
    ElseBody,
}

/// Lowers a `Legacy` THEN/END if-chain into `stmt`'s branches/else. Bodies in
/// this form may be a `Block`, a bare `RETURN`/`THROW`, or a value/subquery —
/// each is wrapped into a single-statement [`Block`] so both forms share the
/// branch/else shape.
fn lower_legacy_if(node: Node<'_>, text: &str, stmt: &mut IfElseStmt) {
    let mut slot = LegacySlot::Condition;
    let mut pending_condition = None;

    for child in named_children(node) {
        if child.kind() == "Keyword" {
            match text[child.byte_range()].to_ascii_uppercase().as_str() {
                "THEN" => slot = LegacySlot::Body,
                "ELSE" => slot = LegacySlot::ElseBody,
                "IF" => slot = LegacySlot::Condition,
                _ => {}
            }
            continue;
        }
        if child.is_error() || child.is_missing() {
            continue;
        }
        match slot {
            LegacySlot::Condition => pending_condition = Some(lower_expr(child, text)),
            LegacySlot::Body => {
                let body = legacy_body_block(child, text);
                if let Some(condition) = pending_condition.take() {
                    stmt.branches.push(IfBranch { condition, body });
                }
            }
            LegacySlot::ElseBody => stmt.else_branch = Some(legacy_body_block(child, text)),
        }
    }
}

/// A `Legacy` branch body as a [`Block`]: a real block passes through; a bare
/// `RETURN`/`THROW` statement or a value/subquery becomes a one-statement block.
fn legacy_body_block(node: Node<'_>, text: &str) -> crate::ast::Block {
    match node.kind() {
        "Block" => super::expr::lower_block_node(node, text),
        "ReturnStatement" | "ThrowStatement" => crate::ast::Block {
            statements: vec![lower_statement(node, text)],
        },
        _ => {
            let expr = lower_expr(node, text);
            let span = expr.span;
            crate::ast::Block {
                statements: vec![Spanned::new(Statement::Expr(expr), span)],
            }
        }
    }
}

fn lower_for(node: Node<'_>, text: &str) -> ForStmt {
    let mut binding = None;
    let mut iterable = None;
    let mut body = None;

    for child in named_children(node) {
        match child.kind() {
            "Keyword" => {}
            "VariableName" if binding.is_none() => {
                binding = Some(Spanned::new(
                    text[child.byte_range()].trim_start_matches('$').to_string(),
                    node_range(child),
                ));
            }
            "Block" => body = Some(super::expr::lower_block_node(child, text)),
            _ if child.is_error() || child.is_missing() => {}
            _ if iterable.is_none() => iterable = Some(lower_expr(child, text)),
            _ => {}
        }
    }

    let span = node_range(node);
    ForStmt {
        binding: binding.unwrap_or_else(|| Spanned::new(String::new(), span)),
        iterable: iterable.unwrap_or_else(|| Spanned::new(Expr::Partial(partial(node)), span)),
        body: body.unwrap_or_default(),
    }
}

fn lower_use(node: Node<'_>, text: &str) -> UseStmt {
    let mut stmt = UseStmt {
        namespace: None,
        database: None,
    };
    let mut slot: Option<&str> = None;

    for child in named_children(node) {
        match child.kind() {
            "Keyword" => {
                let keyword = text[child.byte_range()].to_ascii_lowercase();
                if matches!(keyword.as_str(), "ns" | "namespace") {
                    slot = Some("ns");
                } else if matches!(keyword.as_str(), "db" | "database") {
                    slot = Some("db");
                }
            }
            "Ident" => {
                let name = Spanned::new(text[child.byte_range()].to_string(), node_range(child));
                match slot.take() {
                    Some("ns") => stmt.namespace = Some(name),
                    Some("db") => stmt.database = Some(name),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    stmt
}

fn lower_rebuild(node: Node<'_>, text: &str) -> RebuildStmt {
    let mut stmt = RebuildStmt {
        index: None,
        table: None,
    };
    for child in named_children(node) {
        match child.kind() {
            "Keyword" => {}
            "Ident" if stmt.index.is_none() => {
                stmt.index = Some(Spanned::new(
                    text[child.byte_range()].to_string(),
                    node_range(child),
                ));
            }
            "OnTableClause" => {
                stmt.table = named_children(child)
                    .into_iter()
                    .find(|c| c.kind() == "Ident")
                    .map(|ident| {
                        Spanned::new(text[ident.byte_range()].to_string(), node_range(ident))
                    });
            }
            _ => {}
        }
    }
    stmt
}

/// The single value expression of statements like `RETURN <e>` / `THROW <e>`
/// / `SLEEP <e>` / `KILL <e>`.
fn statement_value_expr(node: Node<'_>, text: &str) -> Option<Spanned<Expr>> {
    named_children(node)
        .into_iter()
        .find(|child| child.kind() != "Keyword" && !child.is_error() && !child.is_missing())
        .map(|value| lower_expr(value, text))
}

/// The table identifier following a keyword (`FROM person`, `TABLE person`).
fn table_after_keyword(node: Node<'_>, text: &str, keyword: &str) -> Option<Spanned<String>> {
    let mut saw_keyword = false;
    for child in named_children(node) {
        if child.kind() == "Keyword" {
            if text[child.byte_range()].eq_ignore_ascii_case(keyword) {
                saw_keyword = true;
            }
            continue;
        }
        if saw_keyword && child.kind() == "Ident" {
            return Some(Spanned::new(
                text[child.byte_range()].to_string(),
                node_range(child),
            ));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Schema statements
// ---------------------------------------------------------------------------

/// The DEFINE kind is the second keyword; Tier 1 kinds are modeled, the
/// long tail is an explicit `Other`.
fn lower_show(node: Node<'_>, text: &str) -> ShowStmt {
    let mut since = None;
    let mut saw_since = false;
    for child in named_children(node) {
        if child.kind() == "Keyword" {
            saw_since = text[child.byte_range()].eq_ignore_ascii_case("since");
            continue;
        }
        if saw_since && matches!(child.kind(), "String" | "Number") {
            since = Some(lower_expr(child, text));
            saw_since = false;
        }
    }
    ShowStmt {
        table: table_after_keyword(node, text, "table"),
        since,
    }
}

fn lower_define(node: Node<'_>, text: &str) -> DefineStmt {
    let kind = named_children(node)
        .into_iter()
        .filter(|child| child.kind() == "Keyword")
        .nth(1)
        .map(|keyword| text[keyword.byte_range()].to_ascii_lowercase())
        .unwrap_or_default();

    match kind.as_str() {
        "table" => DefineStmt::Table(lower_define_table(node, text)),
        "field" => DefineStmt::Field(lower_define_field(node, text)),
        "index" => DefineStmt::Index(lower_define_index(node, text)),
        "event" => DefineStmt::Event(lower_define_event(node, text)),
        "param" => DefineStmt::Param(lower_define_param(node, text)),
        "function" => DefineStmt::Function(lower_define_function(node, text)),
        "analyzer" => DefineStmt::Analyzer(lower_define_analyzer(node, text)),
        _ => DefineStmt::Other(partial(node)),
    }
}

fn spanned_text(node: Node<'_>, text: &str) -> Spanned<String> {
    Spanned::new(text[node.byte_range()].to_string(), node_range(node))
}

fn lower_define_table(node: Node<'_>, text: &str) -> DefineTable {
    let mut def = DefineTable {
        name: Spanned::new(String::new(), node_range(node)),
        overwrite: false,
        schemafull: false,
        relation: None,
        drop: false,
        changefeed: false,
        permissions: Vec::new(),
    };
    let mut named = false;

    for child in named_children(node) {
        match child.kind() {
            "Keyword" => {
                let keyword = text[child.byte_range()].to_ascii_lowercase();
                match keyword.as_str() {
                    "schemafull" => def.schemafull = true,
                    "overwrite" => def.overwrite = true,
                    "drop" => def.drop = true,
                    _ => {}
                }
            }
            "OverwriteClause" => def.overwrite = true,
            "ChangefeedClause" => def.changefeed = true,
            "Ident" if !named => {
                def.name = spanned_text(child, text);
                named = true;
            }
            "TableTypeClause" => def.relation = lower_relation_def(child, text),
            "PermissionsBasicClause" | "PermissionsForClause" => {
                lower_permission_predicates(child, text, &mut def.permissions);
            }
            "CommentClause" | "TableViewClause" | "IfNotExistsClause" => {
                // Recognized but not modeled for type inference.
            }
            _ if child.is_error() || child.is_missing() => {}
            _ => {}
        }
    }

    def
}

/// `TYPE RELATION IN a OUT b` — idents are assigned to the side whose
/// keyword most recently preceded them.
fn lower_relation_def(clause: Node<'_>, text: &str) -> Option<RelationDef> {
    let mut relation = false;
    let mut side: Option<&str> = None;
    let mut def = RelationDef {
        in_tables: Vec::new(),
        out_tables: Vec::new(),
        span: node_range(clause),
    };

    for child in named_children(clause) {
        match child.kind() {
            "Keyword" => {
                let keyword = text[child.byte_range()].to_ascii_lowercase();
                match keyword.as_str() {
                    "relation" => relation = true,
                    "in" | "from" => side = Some("in"),
                    "out" | "to" => side = Some("out"),
                    _ => {}
                }
            }
            "Ident" => match side {
                Some("in") => def.in_tables.push(spanned_text(child, text)),
                Some("out") => def.out_tables.push(spanned_text(child, text)),
                _ => {}
            },
            _ => {}
        }
    }

    relation.then_some(def)
}

fn on_table_ident(node: Node<'_>, text: &str) -> Option<Spanned<String>> {
    named_children(node)
        .into_iter()
        .find(|child| child.kind() == "OnTableClause")
        .and_then(|clause| {
            named_children(clause)
                .into_iter()
                .find(|child| child.kind() == "Ident")
        })
        .map(|ident| spanned_text(ident, text))
}

fn lower_define_field(node: Node<'_>, text: &str) -> DefineField {
    let span = node_range(node);
    let mut def = DefineField {
        path: Spanned::new(Idiom { parts: Vec::new() }, span),
        table: Spanned::new(String::new(), span),
        ty: None,
        overwrite: false,
        default: None,
        value: None,
        computed: None,
        reference: false,
        assert: None,
        readonly: false,
        permissions: Vec::new(),
    };

    for child in named_children(node) {
        match child.kind() {
            "Keyword" if text[child.byte_range()].eq_ignore_ascii_case("overwrite") => {
                def.overwrite = true;
            }
            "Keyword" => {}
            "Idiom" => {
                def.path = Spanned::new(
                    super::expr::lower_idiom_node(child, text),
                    node_range(child),
                );
            }
            "OnTableClause" => {
                if let Some(table) = on_table_ident(node, text) {
                    def.table = table;
                }
            }
            "DefaultClause" => def.default = clause_expr(child, text),
            "ValueClause" => def.value = clause_expr(child, text),
            "ComputedClause" => def.computed = clause_expr(child, text),
            "AssertClause" => def.assert = clause_expr(child, text),
            "ReadonlyClause" => def.readonly = true,
            "TypeClause" => {
                def.ty = named_children(child)
                    .into_iter()
                    .find(|c| {
                        matches!(
                            c.kind(),
                            "Type"
                                | "TypeName"
                                | "ParameterizedType"
                                | "UnionType"
                                | "LiteralType"
                                | "ArrayType"
                                | "ObjectType"
                        )
                    })
                    .map(|ty| super::expr::lower_type_expr(ty, text));
            }
            "PermissionsBasicClause" | "PermissionsForClause" => {
                lower_permission_predicates(child, text, &mut def.permissions);
            }
            "ReferenceClause" => def.reference = true,
            "CommentClause" => {}
            _ if child.is_error() || child.is_missing() => {}
            _ => {}
        }
    }

    def
}

fn lower_define_index(node: Node<'_>, text: &str) -> DefineIndex {
    let span = node_range(node);
    let mut def = DefineIndex {
        name: Spanned::new(String::new(), span),
        table: Spanned::new(String::new(), span),
        fields: Vec::new(),
        kind: IndexKind::Normal,
    };
    let mut named = false;

    for child in named_children(node) {
        match child.kind() {
            "Keyword" => {}
            "Ident" if !named => {
                def.name = spanned_text(child, text);
                named = true;
            }
            "OnTableClause" => {
                if let Some(table) = on_table_ident(node, text) {
                    def.table = table;
                }
            }
            "FieldsColumnsClause" => def.fields = clause_idioms(child, text),
            "IndexClause" => def.kind = index_kind_from_clause(child),
            "UniqueClause" => def.kind = IndexKind::Unique,
            _ => {}
        }
    }

    def
}

/// The backing structure named inside an `IndexClause`: `SEARCH ANALYZER`
/// and its 3.0 spelling `FULLTEXT ANALYZER` are full-text, `MTREE`/`HNSW`
/// are vector, `UNIQUE` is a constraint, and a clause with none of these is
/// a plain index.
fn index_kind_from_clause(clause: Node<'_>) -> IndexKind {
    for child in named_children(clause) {
        match child.kind() {
            "SearchAnalyzerClause" | "FulltextClause" => return IndexKind::Search,
            "MtreeClause" | "HnswClause" => return IndexKind::Vector,
            "UniqueClause" => return IndexKind::Unique,
            _ => {}
        }
    }
    IndexKind::Normal
}

fn lower_define_event(node: Node<'_>, text: &str) -> DefineEvent {
    let span = node_range(node);
    let mut def = DefineEvent {
        name: Spanned::new(String::new(), span),
        table: Spanned::new(String::new(), span),
        when: None,
        then: None,
    };
    let mut named = false;

    for child in named_children(node) {
        match child.kind() {
            "Keyword" => {}
            "Ident" if !named => {
                def.name = spanned_text(child, text);
                named = true;
            }
            "OnTableClause" => {
                if let Some(table) = on_table_ident(node, text) {
                    def.table = table;
                }
            }
            "WhenClause" => def.when = clause_expr(child, text),
            "ThenClause" => def.then = clause_expr(child, text),
            _ => {}
        }
    }

    def
}

fn lower_define_param(node: Node<'_>, text: &str) -> DefineParam {
    let span = node_range(node);
    let mut def = DefineParam {
        name: Spanned::new(String::new(), span),
        value: None,
    };

    for child in named_children(node) {
        match child.kind() {
            "Keyword" => {}
            "VariableName" => {
                def.name = Spanned::new(
                    text[child.byte_range()].trim_start_matches('$').to_string(),
                    node_range(child),
                );
            }
            _ if child.is_error() || child.is_missing() => {}
            _ if def.value.is_none() => def.value = Some(lower_expr(child, text)),
            _ => {}
        }
    }

    def
}

fn lower_define_function(node: Node<'_>, text: &str) -> DefineFunction {
    let span = node_range(node);
    let mut def = DefineFunction {
        name: Spanned::new(String::new(), span),
        params: Vec::new(),
        body: None,
        return_ty: None,
    };

    let mut saw_arrow = false;
    for child in named_children(node) {
        match child.kind() {
            "Keyword" => {}
            "LookupRight" => saw_arrow = true,
            "Type" | "TypeName" | "ParameterizedType" | "UnionType" | "LiteralType"
                if saw_arrow && def.return_ty.is_none() =>
            {
                def.return_ty = Some(super::expr::lower_type_expr(child, text));
            }
            "FunctionName" => def.name = spanned_text(child, text),
            "ParamDefinition" => {
                let mut name = None;
                let mut ty = None;
                for part in named_children(child) {
                    match part.kind() {
                        "VariableName" => {
                            name = Some(Spanned::new(
                                text[part.byte_range()].trim_start_matches('$').to_string(),
                                node_range(part),
                            ));
                        }
                        "Type" | "TypeName" | "ParameterizedType" | "UnionType" | "LiteralType" => {
                            ty = Some(super::expr::lower_type_expr(part, text));
                        }
                        _ => {}
                    }
                }
                if let Some(name) = name {
                    def.params.push((name, ty));
                }
            }
            "Block" => def.body = Some(super::expr::lower_block_node(child, text)),
            _ => {}
        }
    }

    def
}

fn lower_define_analyzer(node: Node<'_>, text: &str) -> DefineAnalyzer {
    let span = node_range(node);
    let mut def = DefineAnalyzer {
        name: Spanned::new(String::new(), span),
        tokenizers: Vec::new(),
        filters: Vec::new(),
    };
    let mut named = false;

    for child in named_children(node) {
        match child.kind() {
            "Keyword" => {}
            "Ident" if !named => {
                def.name = spanned_text(child, text);
                named = true;
            }
            "TokenizersClause" => {
                def.tokenizers = named_children(child)
                    .into_iter()
                    .filter(|c| c.kind() == "AnalyzerTokenizer")
                    .map(|c| spanned_text(c, text))
                    .collect();
            }
            "FiltersClause" => {
                // Each `AnalyzerFilters` node is one whole filter with its
                // arguments (`snowball(english)`); the inner `Filter` token
                // drops the argument.
                def.filters = named_children(child)
                    .into_iter()
                    .filter(|c| c.kind() == "AnalyzerFilters")
                    .map(|c| spanned_text(c, text))
                    .collect();
            }
            _ => {}
        }
    }

    def
}

fn lower_remove(node: Node<'_>, text: &str) -> RemoveStmt {
    let kind = named_children(node)
        .into_iter()
        .filter(|child| child.kind() == "Keyword")
        .nth(1)
        .map(|keyword| text[keyword.byte_range()].to_ascii_lowercase())
        .unwrap_or_default();
    let first_ident = named_children(node)
        .into_iter()
        .find(|child| child.kind() == "Ident");
    let table = on_table_ident(node, text);

    for child in named_children(node) {
        if child.is_error() || child.is_missing() {}
    }

    let span = node_range(node);
    let target = match (kind.as_str(), first_ident, table) {
        ("table", Some(name), _) => RemoveTarget::Table(spanned_text(name, text)),
        ("field", Some(name), Some(table)) => RemoveTarget::Field {
            field: Spanned::new(super::expr::lower_idiom_node(name, text), node_range(name)),
            table,
        },
        ("index", Some(name), Some(table)) => RemoveTarget::Index {
            index: spanned_text(name, text),
            table,
        },
        _ => RemoveTarget::Other(crate::ast::PartialNode {
            span,
            cst_kind: format!("RemoveStatement:{kind}"),
        }),
    };

    RemoveStmt { target }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::IdiomPart;

    use crate::parse::{parse_source, ParsedSource};
    use crate::source::SourceId;

    fn parse(query: &str) -> ParsedSource {
        parse_source(SourceId::new("lower:select"), query).expect("query parses")
    }

    fn lower_select_stmt(parsed: &ParsedSource) -> SelectStmt {
        let node = find_first(parsed.tree().root_node(), "SelectStatement")
            .expect("select statement exists");
        match lower_statement(node, parsed.text()).node {
            Statement::Select(stmt) => stmt,
            other => panic!("expected select, got {other:?}"),
        }
    }

    fn find_first<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
        if node.kind() == kind {
            return Some(node);
        }
        let mut cursor = node.walk();
        let found = node
            .children(&mut cursor)
            .find_map(|child| find_first(child, kind));
        found
    }

    #[test]
    fn lowers_projections_sources_and_flags() {
        let parsed = parse("SELECT name AS display, age, * FROM ONLY person:one;");
        let stmt = lower_select_stmt(&parsed);

        assert!(stmt.only);
        assert!(!stmt.value);
        assert_eq!(stmt.projections.len(), 3);
        let Projection::Expr { expr, alias } = &stmt.projections[0] else {
            panic!("expected expr projection");
        };
        assert!(matches!(expr.node, Expr::Idiom(_)));
        assert_eq!(alias.as_ref().map(|a| a.node.as_str()), Some("display"));
        assert!(matches!(stmt.projections[2], Projection::Wildcard(_)));

        assert_eq!(stmt.from.len(), 1);
        let Expr::RecordId { table, .. } = &stmt.from[0].node else {
            panic!("expected record id source, got {:?}", stmt.from[0].node);
        };
        assert_eq!(table.node, "person");
    }

    #[test]
    fn lowers_value_flag_and_multiple_sources() {
        let parsed = parse("SELECT VALUE age FROM person, company;");
        let stmt = lower_select_stmt(&parsed);

        assert!(stmt.value);
        assert_eq!(stmt.from.len(), 2);
        assert!(matches!(&stmt.from[0].node, Expr::Table(t) if t.node == "person"));
        assert!(matches!(&stmt.from[1].node, Expr::Table(t) if t.node == "company"));
    }

    #[test]
    fn lowers_modifier_clauses_including_combo_flattening() {
        let parsed = parse(
            "SELECT * OMIT password FROM person WHERE age > 18 GROUP BY city ORDER BY name DESC, age LIMIT 5 START 10 FETCH profile SPLIT tags TIMEOUT 5s PARALLEL EXPLAIN;",
        );
        let stmt = lower_select_stmt(&parsed);

        assert_eq!(stmt.omit.len(), 1);
        assert_eq!(stmt.fetch.len(), 1);
        assert_eq!(stmt.split.len(), 1);
        assert!(stmt.where_clause.is_some());

        let group = stmt.group.expect("group clause");
        assert!(!group.all);
        assert_eq!(group.keys.len(), 1);

        let order = stmt.order.expect("order clause");
        assert_eq!(order.keys.len(), 2);
        assert!(order.keys[0].descending);
        assert!(!order.keys[1].descending);

        let limit = stmt.limit.expect("limit");
        assert_eq!(limit.node, Expr::Literal(Literal::Int(5)));
        let start = stmt.start.expect("start");
        assert_eq!(start.node, Expr::Literal(Literal::Int(10)));

        assert!(stmt.timeout.is_some());
        assert!(stmt.parallel.is_some());
        assert!(stmt.explain.is_some());
    }

    #[test]
    fn lowers_group_all() {
        let parsed = parse("SELECT * FROM person GROUP ALL;");
        let stmt = lower_select_stmt(&parsed);

        let group = stmt.group.expect("group clause");
        assert!(group.all);
        assert!(group.keys.is_empty());
    }

    #[test]
    fn lowers_graph_source_as_idiom() {
        let parsed = parse("SELECT * FROM person->likes->post;");
        let stmt = lower_select_stmt(&parsed);

        let Expr::Idiom(idiom) = &stmt.from[0].node else {
            panic!("expected idiom source, got {:?}", stmt.from[0].node);
        };
        assert!(matches!(idiom.parts[0].node, IdiomPart::Field(_)));
        assert!(matches!(idiom.parts[1].node, IdiomPart::Graph { .. }));
    }

    #[test]
    fn lowers_param_and_subquery_sources() {
        let parsed = parse("SELECT * FROM $tbl;");
        let stmt = lower_select_stmt(&parsed);
        assert!(matches!(&stmt.from[0].node, Expr::Param(p) if p == "tbl"));

        let parsed = parse("SELECT * FROM (SELECT * FROM person);");
        let stmt = lower_select_stmt(&parsed);
        let Expr::Subquery(inner) = &stmt.from[0].node else {
            panic!("expected subquery source, got {:?}", stmt.from[0].node);
        };
        assert!(matches!(inner.node, Statement::Select(_)));
    }

    #[test]
    fn type_irrelevant_clauses_do_not_disturb_lowering() {
        // The permissive grammar allows clauses a statement doesn't take
        // (RETURN on SELECT) and clauses that can't affect the response type
        // (TIMEOUT on CREATE). Neither disturbs the lowered structure.
        let parsed = parse("SELECT * FROM person RETURN NONE;");
        let stmt = lower_select_stmt(&parsed);
        assert_eq!(stmt.from.len(), 1);
        assert!(matches!(stmt.projections[0], Projection::Wildcard(_)));

        let parsed = parse("CREATE person SET name = 'A' TIMEOUT 5s;");
        let stmt = lower_kind(&parsed, "CreateStatement", |s| match s {
            Statement::Create(stmt) => Some(stmt),
            _ => None,
        });
        assert!(matches!(stmt.data, Some(DataClause::Set(_))));
    }

    #[test]
    fn statements_with_broken_syntax_lower_to_partial() {
        // The recovered parse of `SET = 5` contains a zero-width missing
        // identifier; the statement must not present a half-parsed
        // structure to analyzers.
        let parsed = parse("CREATE person SET = 5;");
        let node = find_first(parsed.tree().root_node(), "CreateStatement")
            .expect("create statement exists");
        let lowered = lower_statement(node, parsed.text());

        assert!(matches!(lowered.node, Statement::Partial(_)));
    }

    fn lower_kind<T>(
        parsed: &ParsedSource,
        node_kind: &str,
        extract: impl Fn(Statement) -> Option<T>,
    ) -> T {
        let node = find_first(parsed.tree().root_node(), node_kind)
            .unwrap_or_else(|| panic!("no {node_kind} in {:?}", parsed.text()));
        extract(lower_statement(node, parsed.text()).node)
            .unwrap_or_else(|| panic!("unexpected statement variant"))
    }

    #[test]
    fn lowers_create_with_only_record_target_set_data_and_return_mode() {
        let parsed = parse("CREATE ONLY person:one SET name = 'A', age += 1 RETURN AFTER;");
        let stmt = lower_kind(&parsed, "CreateStatement", |s| match s {
            Statement::Create(stmt) => Some(stmt),
            _ => None,
        });

        assert!(stmt.only);
        assert!(matches!(
            &stmt.targets[0].node,
            Expr::RecordId { table, .. } if table.node == "person"
        ));
        let Some(DataClause::Set(assignments)) = &stmt.data else {
            panic!("expected SET data, got {:?}", stmt.data);
        };
        assert_eq!(assignments.len(), 2);
        assert_eq!(assignments[0].op.node, AssignOp::Assign);
        assert_eq!(assignments[1].op.node, AssignOp::Add);
        assert_eq!(stmt.ret.as_ref().map(|r| &r.node), Some(&ReturnMode::After));
    }

    #[test]
    fn lowers_update_with_content_where_and_diff_return() {
        let parsed = parse("UPDATE person CONTENT { name: 'B' } WHERE age > 18 RETURN DIFF;");
        let stmt = lower_kind(&parsed, "UpdateStatement", |s| match s {
            Statement::Update(stmt) => Some(stmt),
            _ => None,
        });

        assert!(matches!(&stmt.targets[0].node, Expr::Table(t) if t.node == "person"));
        assert!(matches!(stmt.data, Some(DataClause::Content(_))));
        assert!(stmt.where_clause.is_some());
        assert_eq!(stmt.ret.map(|r| r.node), Some(ReturnMode::Diff));
    }

    #[test]
    fn return_mode_classification_is_structural_not_substring() {
        // Classification must be structural: substring matching on the
        // clause text misreads field names that contain keyword substrings.
        let cases = [
            ("UPDATE person RETURN NONE;", ReturnMode::None),
            ("UPDATE person RETURN BEFORE;", ReturnMode::Before),
            ("UPDATE person RETURN AFTER;", ReturnMode::After),
            ("UPDATE person RETURN DIFF;", ReturnMode::Diff),
        ];
        for (query, expected) in cases {
            let parsed = parse(query);
            let stmt = lower_kind(&parsed, "UpdateStatement", |s| match s {
                Statement::Update(stmt) => Some(stmt),
                _ => None,
            });
            assert_eq!(stmt.ret.map(|r| r.node), Some(expected), "query: {query}");
        }

        // Field names containing keyword substrings are field returns.
        for query in [
            "UPDATE person RETURN nonexistent_field;",
            "UPDATE person RETURN difference;",
            "UPDATE person RETURN before_state;",
        ] {
            let parsed = parse(query);
            let stmt = lower_kind(&parsed, "UpdateStatement", |s| match s {
                Statement::Update(stmt) => Some(stmt),
                _ => None,
            });
            assert!(
                matches!(
                    stmt.ret.as_ref().map(|r| &r.node),
                    Some(ReturnMode::Fields(_))
                ),
                "query {query} must classify as Fields, got {:?}",
                stmt.ret
            );
        }
    }

    #[test]
    fn lowers_tuple_insert_into_column_value_pairs() {
        let parsed = parse("INSERT IGNORE INTO person (name, age) VALUES ('A', 1), ('B', 2);");
        let stmt = lower_kind(&parsed, "InsertStatement", |s| match s {
            Statement::Insert(stmt) => Some(stmt),
            _ => None,
        });

        assert!(stmt.ignore);
        assert!(matches!(
            stmt.target.as_ref().map(|t| &t.node),
            Some(Expr::Table(t)) if t.node == "person"
        ));
        let InsertData::Rows { rows, .. } = &stmt.data else {
            panic!("expected rows, got {:?}", stmt.data);
        };
        assert_eq!(rows.len(), 2);
        for row in rows {
            assert_eq!(row.len(), 2);
            assert!(matches!(&row[0].0.node.parts[0].node, IdiomPart::Field(f) if f == "name"));
            assert!(matches!(&row[1].0.node.parts[0].node, IdiomPart::Field(f) if f == "age"));
        }
        assert_eq!(rows[0][1].1.node, Expr::Literal(Literal::Int(1)));
        assert_eq!(rows[1][1].1.node, Expr::Literal(Literal::Int(2)));
    }

    #[test]
    fn lowers_object_insert_as_values_payload() {
        let parsed = parse("INSERT INTO person { name: 'A' };");
        let stmt = lower_kind(&parsed, "InsertStatement", |s| match s {
            Statement::Insert(stmt) => Some(stmt),
            _ => None,
        });

        let InsertData::Values(values) = &stmt.data else {
            panic!("expected values payload, got {:?}", stmt.data);
        };
        assert_eq!(values.len(), 1);
        assert!(matches!(values[0].node, Expr::Object(_)));
    }

    #[test]
    fn lowers_bulk_insert_array_as_an_array_of_row_objects() {
        // The bracketed payload is its own `BulkInsert` grammar node; before
        // it was lowered the whole payload vanished and nothing about those
        // rows could be checked.
        let parsed = parse("INSERT INTO person [{ name: 'A' }, { name: 'B' }];");
        let stmt = lower_kind(&parsed, "InsertStatement", |s| match s {
            Statement::Insert(stmt) => Some(stmt),
            _ => None,
        });

        let InsertData::Values(values) = &stmt.data else {
            panic!("expected values payload, got {:?}", stmt.data);
        };
        assert_eq!(values.len(), 1);
        let Expr::Array(rows) = &values[0].node else {
            panic!("expected an array payload, got {:?}", values[0].node);
        };
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| matches!(row.node, Expr::Object(_))));
    }

    #[test]
    fn lowers_relate_endpoints_in_order() {
        let parsed =
            parse("RELATE ONLY person:one->likes->post:two SET strength = 0.5 RETURN NONE;");
        let stmt = lower_kind(&parsed, "RelateStatement", |s| match s {
            Statement::Relate(stmt) => Some(stmt),
            _ => None,
        });

        assert!(stmt.only);
        assert!(matches!(
            stmt.from.as_ref().map(|s| &s.node),
            Some(Expr::RecordId { table, .. }) if table.node == "person"
        ));
        assert!(matches!(
            stmt.edge.as_ref().map(|s| &s.node),
            Some(Expr::Table(t)) if t.node == "likes"
        ));
        assert!(matches!(
            stmt.to.as_ref().map(|s| &s.node),
            Some(Expr::RecordId { table, .. }) if table.node == "post"
        ));
        assert!(matches!(stmt.data, Some(DataClause::Set(_))));
        assert_eq!(stmt.ret.map(|r| r.node), Some(ReturnMode::None));
    }

    #[test]
    fn lowers_let_return_and_for_statements() {
        let parsed = parse("LET $age = 42;");
        let stmt = lower_kind(&parsed, "LetStatement", |s| match s {
            Statement::Let(stmt) => Some(stmt),
            _ => None,
        });
        assert_eq!(stmt.name.node, "age");
        assert_eq!(stmt.value.node, Expr::Literal(Literal::Int(42)));

        let parsed = parse("RETURN $age + 1;");
        let stmt = lower_kind(&parsed, "ReturnStatement", |s| match s {
            Statement::Return(stmt) => Some(stmt),
            _ => None,
        });
        assert!(matches!(
            stmt.value.as_ref().map(|v| &v.node),
            Some(Expr::Binary { .. })
        ));

        let parsed = parse("FOR $item IN [1, 2] { UPDATE person SET n = $item; };");
        let stmt = lower_kind(&parsed, "ForStatement", |s| match s {
            Statement::For(stmt) => Some(stmt),
            _ => None,
        });
        assert_eq!(stmt.binding.node, "item");
        assert!(matches!(stmt.iterable.node, Expr::Array(_)));
        assert_eq!(stmt.body.statements.len(), 1);
        assert!(matches!(stmt.body.statements[0].node, Statement::Update(_)));
    }

    #[test]
    fn lowers_if_else_chains_with_conditions_bodies_and_else() {
        let parsed = parse(
            "IF $x > 1 { RETURN 'big'; } ELSE IF $x > 0 { RETURN 'small'; } ELSE { RETURN 'neg'; };",
        );
        let stmt = lower_kind(&parsed, "IfElseStatement", |s| match s {
            Statement::IfElse(stmt) => Some(stmt),
            _ => None,
        });

        assert_eq!(stmt.branches.len(), 2);
        for branch in &stmt.branches {
            assert!(matches!(branch.condition.node, Expr::Binary { .. }));
            assert_eq!(branch.body.statements.len(), 1);
        }
        let else_branch = stmt.else_branch.expect("else branch");
        assert!(matches!(
            else_branch.statements[0].node,
            Statement::Return(_)
        ));
    }

    #[test]
    fn lowers_thin_statements_with_their_table_references() {
        let parsed = parse("LIVE SELECT * FROM person;");
        let stmt = lower_kind(&parsed, "LiveSelectStatement", |s| match s {
            Statement::LiveSelect(stmt) => Some(stmt),
            _ => None,
        });
        assert_eq!(stmt.table.as_ref().map(|t| t.node.as_str()), Some("person"));

        let parsed = parse("REBUILD INDEX idx ON person;");
        let stmt = lower_kind(&parsed, "RebuildStatement", |s| match s {
            Statement::Rebuild(stmt) => Some(stmt),
            _ => None,
        });
        assert_eq!(stmt.index.as_ref().map(|i| i.node.as_str()), Some("idx"));
        assert_eq!(stmt.table.as_ref().map(|t| t.node.as_str()), Some("person"));

        let parsed = parse("USE NS prod DB main;");
        let stmt = lower_kind(&parsed, "UseStatement", |s| match s {
            Statement::Use(stmt) => Some(stmt),
            _ => None,
        });
        assert_eq!(
            stmt.namespace.as_ref().map(|n| n.node.as_str()),
            Some("prod")
        );
        assert_eq!(
            stmt.database.as_ref().map(|d| d.node.as_str()),
            Some("main")
        );
    }

    #[test]
    fn lowers_define_field_with_structured_types() {
        let parsed = parse("DEFINE FIELD tags ON person TYPE array<string>;");
        let stmt = lower_kind(&parsed, "DefineStatement", |s| match s {
            Statement::Define(DefineStmt::Field(def)) => Some(def),
            _ => None,
        });
        assert_eq!(stmt.table.node, "person");
        assert!(matches!(&stmt.path.node.parts[0].node, IdiomPart::Field(f) if f == "tags"));
        let Some(ty) = &stmt.ty else {
            panic!("expected a type");
        };
        let crate::ast::TypeExpr::Parameterized { name, args } = &ty.node else {
            panic!("expected parameterized type, got {:?}", ty.node);
        };
        assert_eq!(name.node, "array");
        assert!(matches!(&args[0].node, crate::ast::TypeExpr::Name(n) if n.node == "string"));

        // option<T> normalizes to Optional; literal unions become Literal types.
        let parsed = parse("DEFINE FIELD age ON person TYPE option<int>;");
        let stmt = lower_kind(&parsed, "DefineStatement", |s| match s {
            Statement::Define(DefineStmt::Field(def)) => Some(def),
            _ => None,
        });
        assert!(matches!(
            stmt.ty.as_ref().map(|t| &t.node),
            Some(crate::ast::TypeExpr::Optional(_))
        ));

        let parsed = parse("DEFINE FIELD status ON person TYPE 'a' | 'b';");
        let stmt = lower_kind(&parsed, "DefineStatement", |s| match s {
            Statement::Define(DefineStmt::Field(def)) => Some(def),
            _ => None,
        });
        let Some(crate::ast::TypeExpr::Union(variants)) = stmt.ty.as_ref().map(|t| &t.node) else {
            panic!("expected union type");
        };
        assert!(matches!(
            &variants[0].node,
            crate::ast::TypeExpr::Literal(Literal::String(s)) if s == "a"
        ));
    }

    #[test]
    fn lowers_define_field_computed_reference_back_traversal() {
        // The `COMPUTED` clause is surfaced as an idiom expression, and a `<~`
        // step is marked as a reference traversal (distinct from a `<-` edge).
        let parsed = parse("DEFINE FIELD teams ON organization COMPUTED <~team;");
        let stmt = lower_kind(&parsed, "DefineStatement", |s| match s {
            Statement::Define(DefineStmt::Field(def)) => Some(def),
            _ => None,
        });
        assert!(stmt.value.is_none(), "COMPUTED is not VALUE");
        let computed = stmt.computed.expect("computed clause surfaced");
        let crate::ast::Expr::Idiom(idiom) = &computed.node else {
            panic!("expected idiom, got {:?}", computed.node);
        };
        let IdiomPart::Graph { dir, step } = &idiom.parts[0].node else {
            panic!("expected graph part, got {:?}", idiom.parts[0].node);
        };
        assert_eq!(dir.node, crate::ast::GraphDir::In);
        assert!(step.reference, "`<~` is a reference traversal");
        assert_eq!(step.targets[0].node, "team");
    }

    #[test]
    fn lowers_define_field_reference_flag() {
        let parsed = parse("DEFINE FIELD org ON team TYPE record<organization> REFERENCE;");
        let stmt = lower_kind(&parsed, "DefineStatement", |s| match s {
            Statement::Define(DefineStmt::Field(def)) => Some(def),
            _ => None,
        });
        assert!(stmt.reference, "REFERENCE clause sets the flag");

        // A plain `<-` edge step is NOT a reference traversal.
        let parsed = parse("DEFINE FIELD likes ON person COMPUTED <-likes;");
        let stmt = lower_kind(&parsed, "DefineStatement", |s| match s {
            Statement::Define(DefineStmt::Field(def)) => Some(def),
            _ => None,
        });
        let computed = stmt.computed.expect("computed clause surfaced");
        let crate::ast::Expr::Idiom(idiom) = &computed.node else {
            panic!("expected idiom");
        };
        let IdiomPart::Graph { step, .. } = &idiom.parts[0].node else {
            panic!("expected graph part");
        };
        assert!(!step.reference, "`<-` is an edge traversal, not a reference");
    }

    #[test]
    fn lowers_define_table_with_relation_endpoints() {
        let parsed = parse("DEFINE TABLE likes TYPE RELATION IN person OUT post;");
        let stmt = lower_kind(&parsed, "DefineStatement", |s| match s {
            Statement::Define(DefineStmt::Table(def)) => Some(def),
            _ => None,
        });
        assert_eq!(stmt.name.node, "likes");
        let relation = stmt.relation.expect("relation def");
        assert_eq!(relation.in_tables[0].node, "person");
        assert_eq!(relation.out_tables[0].node, "post");
    }

    #[test]
    fn lowers_define_table_drop_and_changefeed_flags() {
        let parsed = parse("DEFINE TABLE evt DROP CHANGEFEED 3d;");
        let stmt = lower_kind(&parsed, "DefineStatement", |s| match s {
            Statement::Define(DefineStmt::Table(def)) => Some(def),
            _ => None,
        });
        assert_eq!(stmt.name.node, "evt");
        assert!(stmt.drop);
        assert!(stmt.changefeed);

        let parsed = parse("DEFINE TABLE plain;");
        let stmt = lower_kind(&parsed, "DefineStatement", |s| match s {
            Statement::Define(DefineStmt::Table(def)) => Some(def),
            _ => None,
        });
        assert!(!stmt.drop);
        assert!(!stmt.changefeed);
    }

    #[test]
    fn lowers_define_index_backing_kinds() {
        use crate::ast::IndexKind;
        let cases = [
            ("DEFINE INDEX i ON person FIELDS name;", IndexKind::Normal),
            (
                "DEFINE INDEX i ON person FIELDS name UNIQUE;",
                IndexKind::Unique,
            ),
            (
                "DEFINE INDEX i ON person FIELDS body SEARCH ANALYZER ascii;",
                IndexKind::Search,
            ),
            // 3.0 renamed the clause; both spell the same backing structure.
            (
                "DEFINE INDEX i ON person FIELDS body FULLTEXT ANALYZER ascii;",
                IndexKind::Search,
            ),
            (
                "DEFINE INDEX i ON person FIELDS vec MTREE DIMENSION 4;",
                IndexKind::Vector,
            ),
            (
                "DEFINE INDEX i ON person FIELDS vec HNSW DIMENSION 4;",
                IndexKind::Vector,
            ),
        ];
        for (query, expected) in cases {
            let parsed = parse(query);
            let stmt = lower_kind(&parsed, "DefineStatement", |s| match s {
                Statement::Define(DefineStmt::Index(def)) => Some(def),
                _ => None,
            });
            assert_eq!(stmt.kind, expected, "query: {query}");
        }
    }

    #[test]
    fn lowers_define_index_event_and_remove_targets() {
        let parsed = parse("DEFINE INDEX idx ON person FIELDS email, profile.name UNIQUE;");
        let stmt = lower_kind(&parsed, "DefineStatement", |s| match s {
            Statement::Define(DefineStmt::Index(def)) => Some(def),
            _ => None,
        });
        assert_eq!(stmt.name.node, "idx");
        assert_eq!(stmt.table.node, "person");
        assert_eq!(stmt.fields.len(), 2);
        assert_eq!(stmt.kind, crate::ast::IndexKind::Unique);

        let parsed = parse("REMOVE INDEX idx ON person;");
        let stmt = lower_kind(&parsed, "RemoveStatement", |s| match s {
            Statement::Remove(stmt) => Some(stmt),
            _ => None,
        });
        let RemoveTarget::Index { index, table } = &stmt.target else {
            panic!("expected index target, got {:?}", stmt.target);
        };
        assert_eq!(index.node, "idx");
        assert_eq!(table.node, "person");
    }

    #[test]
    fn lowers_delete_with_where_and_return_none() {
        let parsed = parse("DELETE person WHERE age > 18 RETURN NONE;");
        let stmt = lower_kind(&parsed, "DeleteStatement", |s| match s {
            Statement::Delete(stmt) => Some(stmt),
            _ => None,
        });

        assert!(matches!(&stmt.targets[0].node, Expr::Table(t) if t.node == "person"));
        let cond = stmt.where_clause.expect("where clause");
        assert!(matches!(cond.node, Expr::Binary { .. }));
        assert_eq!(stmt.ret.map(|r| r.node), Some(ReturnMode::None));
    }

    #[test]
    fn lowers_upsert_with_set_data() {
        let parsed = parse("UPSERT person:one SET name = 'Ada', age += 1;");
        let stmt = lower_kind(&parsed, "UpsertStatement", |s| match s {
            Statement::Upsert(stmt) => Some(stmt),
            _ => None,
        });

        assert!(matches!(
            &stmt.targets[0].node,
            Expr::RecordId { table, .. } if table.node == "person"
        ));
        let Some(DataClause::Set(assignments)) = &stmt.data else {
            panic!("expected SET data, got {:?}", stmt.data);
        };
        assert_eq!(assignments.len(), 2);
        assert_eq!(assignments[0].op.node, AssignOp::Assign);
        assert_eq!(assignments[1].op.node, AssignOp::Add);
    }

    #[test]
    fn lowers_show_changes_with_since() {
        let parsed = parse("SHOW CHANGES FOR TABLE person SINCE 100;");
        let stmt = lower_kind(&parsed, "ShowStatement", |s| match s {
            Statement::Show(stmt) => Some(stmt),
            _ => None,
        });

        assert_eq!(stmt.table.as_ref().map(|t| t.node.as_str()), Some("person"));
        let since = stmt.since.expect("since captured");
        assert_eq!(since.node, Expr::Literal(Literal::Int(100)));
    }

    #[test]
    fn lowers_define_event_with_when_and_then() {
        let parsed =
            parse("DEFINE EVENT audit ON person WHEN $event = 'CREATE' THEN { RETURN 1; };");
        let stmt = lower_kind(&parsed, "DefineStatement", |s| match s {
            Statement::Define(DefineStmt::Event(def)) => Some(def),
            _ => None,
        });

        assert_eq!(stmt.name.node, "audit");
        assert_eq!(stmt.table.node, "person");
        let when = stmt.when.expect("when clause");
        assert!(matches!(when.node, Expr::Binary { .. }));
        let then = stmt.then.expect("then clause");
        assert!(matches!(then.node, Expr::Block(_)));
    }

    #[test]
    fn lowers_define_param_with_name_and_value() {
        let parsed = parse("DEFINE PARAM $threshold VALUE 5;");
        let stmt = lower_kind(&parsed, "DefineStatement", |s| match s {
            Statement::Define(DefineStmt::Param(def)) => Some(def),
            _ => None,
        });

        assert_eq!(stmt.name.node, "threshold");
        let value = stmt.value.expect("value clause");
        assert_eq!(value.node, Expr::Literal(Literal::Int(5)));
    }

    #[test]
    fn lowers_define_function_with_typed_params_and_return_type() {
        let parsed = parse("DEFINE FUNCTION fn::greet($name: string) -> string { RETURN $name; };");
        let stmt = lower_kind(&parsed, "DefineStatement", |s| match s {
            Statement::Define(DefineStmt::Function(def)) => Some(def),
            _ => None,
        });

        assert_eq!(stmt.name.node, "fn::greet");
        assert_eq!(stmt.params.len(), 1);
        assert_eq!(stmt.params[0].0.node, "name");
        assert!(matches!(
            stmt.params[0].1.as_ref().map(|t| &t.node),
            Some(crate::ast::TypeExpr::Name(n)) if n.node == "string"
        ));
        assert!(matches!(
            stmt.return_ty.as_ref().map(|t| &t.node),
            Some(crate::ast::TypeExpr::Name(n)) if n.node == "string"
        ));
        assert!(stmt.body.is_some());
    }

    #[test]
    fn lowers_define_analyzer_with_tokenizers() {
        let parsed = parse("DEFINE ANALYZER myan TOKENIZERS blank, class;");
        let stmt = lower_kind(&parsed, "DefineStatement", |s| match s {
            Statement::Define(DefineStmt::Analyzer(def)) => Some(def),
            _ => None,
        });

        assert_eq!(stmt.name.node, "myan");
        let tokenizers: Vec<_> = stmt.tokenizers.iter().map(|t| t.node.as_str()).collect();
        assert_eq!(tokenizers, vec!["blank", "class"]);
    }
}
