//! A read-only traversal of the typed AST.
//!
//! [`Visitor`] has one `visit_*` method per node type; each default
//! implementation recurses through the matching free `walk_*` function (the
//! `syn::visit` pattern). A walker overrides only the node types it acts on
//! and, from inside the override, calls the `walk_*` function to keep
//! descending — so a new AST node has to be added *here*, once, instead of in
//! every hand-rolled walker.
//!
//! Every `walk_*` function matches its enum exhaustively and destructures its
//! struct completely: adding a variant or a field to the AST is a compile
//! error in this module until the traversal accounts for it.
//!
//! Two hooks are not node types:
//!
//! - [`Visitor::visit_table_ref`] fires for every position that *references* a
//!   table by name (`FROM person`, `person:one`, `->likes`, `ON person`, a
//!   relation's `IN`/`OUT` list, `REMOVE TABLE person`, ...). A `DEFINE
//!   TABLE`'s own name is a declaration, not a reference, and is not one.
//! - [`Visitor::visit_row_scope`] brackets the clauses a statement evaluates
//!   against the rows of one table — a SELECT's projections and `WHERE`, a
//!   mutation's `SET`/`WHERE`/`RETURN`, a `DEFINE FIELD`'s `VALUE`/`ASSERT`,
//!   a graph step's inline `WHERE`. A walker that resolves bare field paths
//!   overrides it to track the table in scope; everything else inherits the
//!   default, which simply keeps walking.

use super::{
    AlterStmt, Assignment, BeginStmt, Block, BreakStmt, Call, CancelStmt, Closure, CommitStmt,
    ContinueStmt, CreateStmt, DataClause, DefineAnalyzer, DefineEvent, DefineField, DefineFunction,
    DefineIndex, DefineParam, DefineStmt, DefineTable, DeleteStmt, Expr, ForStmt, GraphStep,
    GroupClause, Idiom, IdiomPart, IfBranch, IfElseStmt, InfoStmt, InsertData, InsertStmt,
    KillStmt, LetStmt, Literal, LiveSelectStmt, OptionStmt, OrderClause, OrderKey, PartialNode,
    Projection, Range, RebuildStmt, RelateStmt, RelationDef, RemoveStmt, RemoveTarget, ReturnMode,
    ReturnStmt, Script, SelectStmt, ShowStmt, SleepStmt, Spanned, Statement, ThrowStmt, TypeExpr,
    UpdateStmt, UpsertStmt, UseStmt,
};

/// A read-only AST traversal. Override the `visit_*` methods for the node
/// types you act on; call the matching `walk_*` function from the override
/// to keep descending. Every default recurses into the complete subtree.
///
/// `Sized` because the defaults hand `self` to the generic `walk_*` functions
/// and [`Visitor::visit_row_scope`] takes a closure over `Self`; nothing
/// needs a `dyn Visitor`.
pub trait Visitor: Sized {
    // --- Hooks that are not node types --------------------------------------

    /// A table referenced by name: a `FROM`/mutation target, a record id's
    /// table, a graph step's edge, a `DEFINE ... ON <table>`, a relation's
    /// `IN`/`OUT` endpoints, `REMOVE TABLE`, `INFO FOR TABLE`, ... Not fired
    /// for a `DEFINE TABLE`'s own name (a declaration). Leaf: nothing to walk.
    fn visit_table_ref(&mut self, _name: &Spanned<String>) {}

    /// A region the lowering could not model (`ERROR` node, unhandled
    /// construct). Fired for every `Partial`/`Other`/`unmodeled` position in
    /// the tree. Leaf: nothing to walk.
    fn visit_partial(&mut self, _partial: &PartialNode) {}

    /// Runs `walk` for the clauses evaluated against the rows of `table` —
    /// `None` when the row source is not a single plain table (several
    /// sources, a subquery, a parameter). The default just runs `walk`; a
    /// walker that resolves bare field paths overrides this to set the table
    /// in scope for the duration and restore the enclosing one afterwards.
    fn visit_row_scope(&mut self, _table: Option<&str>, walk: impl FnOnce(&mut Self)) {
        walk(self);
    }

    // --- Statements --------------------------------------------------------

    /// A whole lowered source file.
    fn visit_script(&mut self, script: &Script) {
        walk_script(self, script);
    }

    /// Any statement; dispatches to the per-kind method.
    fn visit_statement(&mut self, statement: &Spanned<Statement>) {
        walk_statement(self, statement);
    }

    /// A `{ ...; ... }` block (statement position, IF/FOR/function body).
    fn visit_block(&mut self, block: &Block) {
        walk_block(self, block);
    }

    /// `SELECT`.
    fn visit_select(&mut self, select: &SelectStmt) {
        walk_select(self, select);
    }

    /// `CREATE`.
    fn visit_create(&mut self, create: &CreateStmt) {
        walk_create(self, create);
    }

    /// `UPDATE`.
    fn visit_update(&mut self, update: &UpdateStmt) {
        walk_update(self, update);
    }

    /// `UPSERT`.
    fn visit_upsert(&mut self, upsert: &UpsertStmt) {
        walk_upsert(self, upsert);
    }

    /// `DELETE`.
    fn visit_delete(&mut self, delete: &DeleteStmt) {
        walk_delete(self, delete);
    }

    /// `INSERT`.
    fn visit_insert(&mut self, insert: &InsertStmt) {
        walk_insert(self, insert);
    }

    /// An `INSERT`'s payload (`VALUES`, object/array, or unlowered).
    fn visit_insert_data(&mut self, data: &InsertData) {
        walk_insert_data(self, data);
    }

    /// `RELATE`.
    fn visit_relate(&mut self, relate: &RelateStmt) {
        walk_relate(self, relate);
    }

    /// `DEFINE ...`; dispatches to the per-kind method.
    fn visit_define(&mut self, define: &DefineStmt) {
        walk_define(self, define);
    }

    /// `DEFINE TABLE`.
    fn visit_define_table(&mut self, table: &DefineTable) {
        walk_define_table(self, table);
    }

    /// A `DEFINE TABLE`'s `TYPE RELATION IN ... OUT ...` clause.
    fn visit_relation_def(&mut self, relation: &RelationDef) {
        walk_relation_def(self, relation);
    }

    /// `DEFINE FIELD`.
    fn visit_define_field(&mut self, field: &DefineField) {
        walk_define_field(self, field);
    }

    /// `DEFINE INDEX`.
    fn visit_define_index(&mut self, index: &DefineIndex) {
        walk_define_index(self, index);
    }

    /// `DEFINE EVENT`.
    fn visit_define_event(&mut self, event: &DefineEvent) {
        walk_define_event(self, event);
    }

    /// `DEFINE PARAM`.
    fn visit_define_param(&mut self, param: &DefineParam) {
        walk_define_param(self, param);
    }

    /// `DEFINE FUNCTION`.
    fn visit_define_function(&mut self, function: &DefineFunction) {
        walk_define_function(self, function);
    }

    /// `DEFINE ANALYZER`.
    fn visit_define_analyzer(&mut self, analyzer: &DefineAnalyzer) {
        walk_define_analyzer(self, analyzer);
    }

    /// `REMOVE ...`.
    fn visit_remove(&mut self, remove: &RemoveStmt) {
        walk_remove(self, remove);
    }

    /// What a `REMOVE` drops.
    fn visit_remove_target(&mut self, target: &RemoveTarget) {
        walk_remove_target(self, target);
    }

    /// `ALTER TABLE`.
    fn visit_alter(&mut self, alter: &AlterStmt) {
        walk_alter(self, alter);
    }

    /// `LET`.
    fn visit_let(&mut self, let_stmt: &LetStmt) {
        walk_let(self, let_stmt);
    }

    /// `RETURN`.
    fn visit_return(&mut self, ret: &ReturnStmt) {
        walk_return(self, ret);
    }

    /// `IF`/`ELSE IF`/`ELSE`.
    fn visit_if_else(&mut self, if_else: &IfElseStmt) {
        walk_if_else(self, if_else);
    }

    /// One `IF`/`ELSE IF` arm.
    fn visit_if_branch(&mut self, branch: &IfBranch) {
        walk_if_branch(self, branch);
    }

    /// `FOR`.
    fn visit_for(&mut self, for_stmt: &ForStmt) {
        walk_for(self, for_stmt);
    }

    /// `LIVE SELECT`.
    fn visit_live_select(&mut self, live: &LiveSelectStmt) {
        walk_live_select(self, live);
    }

    /// `KILL`.
    fn visit_kill(&mut self, kill: &KillStmt) {
        walk_kill(self, kill);
    }

    /// `USE`.
    fn visit_use(&mut self, use_stmt: &UseStmt) {
        walk_use(self, use_stmt);
    }

    /// `INFO FOR ...`.
    fn visit_info(&mut self, info: &InfoStmt) {
        walk_info(self, info);
    }

    /// `SHOW CHANGES`.
    fn visit_show(&mut self, show: &ShowStmt) {
        walk_show(self, show);
    }

    /// `REBUILD INDEX`.
    fn visit_rebuild(&mut self, rebuild: &RebuildStmt) {
        walk_rebuild(self, rebuild);
    }

    /// `THROW`.
    fn visit_throw(&mut self, throw: &ThrowStmt) {
        walk_throw(self, throw);
    }

    /// `BREAK`.
    fn visit_break(&mut self, break_stmt: &BreakStmt) {
        walk_break(self, break_stmt);
    }

    /// `CONTINUE`.
    fn visit_continue(&mut self, continue_stmt: &ContinueStmt) {
        walk_continue(self, continue_stmt);
    }

    /// `BEGIN`.
    fn visit_begin(&mut self, begin: &BeginStmt) {
        walk_begin(self, begin);
    }

    /// `CANCEL`.
    fn visit_cancel(&mut self, cancel: &CancelStmt) {
        walk_cancel(self, cancel);
    }

    /// `COMMIT`.
    fn visit_commit(&mut self, commit: &CommitStmt) {
        walk_commit(self, commit);
    }

    /// `SLEEP`.
    fn visit_sleep(&mut self, sleep: &SleepStmt) {
        walk_sleep(self, sleep);
    }

    /// `OPTION`.
    fn visit_option(&mut self, option: &OptionStmt) {
        walk_option(self, option);
    }

    // --- Clauses -----------------------------------------------------------

    /// One projection of a SELECT list or a `RETURN <fields>` list.
    fn visit_projection(&mut self, projection: &Projection) {
        walk_projection(self, projection);
    }

    /// A mutation's `RETURN` clause.
    fn visit_return_mode(&mut self, ret: &Spanned<ReturnMode>) {
        walk_return_mode(self, ret);
    }

    /// A mutation's payload (`SET`/`UNSET`/`CONTENT`/`MERGE`/...).
    fn visit_data_clause(&mut self, data: &DataClause) {
        walk_data_clause(self, data);
    }

    /// One `target op value` of a `SET` (or `ON DUPLICATE KEY UPDATE`) list.
    fn visit_assignment(&mut self, assignment: &Assignment) {
        walk_assignment(self, assignment);
    }

    /// `ORDER BY`.
    fn visit_order_clause(&mut self, order: &OrderClause) {
        walk_order_clause(self, order);
    }

    /// One `ORDER BY` key.
    fn visit_order_key(&mut self, key: &OrderKey) {
        walk_order_key(self, key);
    }

    /// `GROUP BY` / `GROUP ALL`.
    fn visit_group_clause(&mut self, group: &GroupClause) {
        walk_group_clause(self, group);
    }

    // --- Expressions -------------------------------------------------------

    /// Any expression; dispatches on the variant.
    fn visit_expr(&mut self, expr: &Spanned<Expr>) {
        walk_expr(self, expr);
    }

    /// A literal value. Leaf.
    fn visit_literal(&mut self, literal: &Literal) {
        walk_literal(self, literal);
    }

    /// A `a..b` range value.
    fn visit_range(&mut self, range: &Range) {
        walk_range(self, range);
    }

    /// A function call.
    fn visit_call(&mut self, call: &Call) {
        walk_call(self, call);
    }

    /// A closure value.
    fn visit_closure(&mut self, closure: &Closure) {
        walk_closure(self, closure);
    }

    /// A field path / graph traversal.
    fn visit_idiom(&mut self, idiom: &Idiom) {
        walk_idiom(self, idiom);
    }

    /// One segment of an idiom.
    fn visit_idiom_part(&mut self, part: &Spanned<IdiomPart>) {
        walk_idiom_part(self, part);
    }

    /// The selection inside one graph step (`->(likes WHERE ...)`).
    fn visit_graph_step(&mut self, step: &GraphStep) {
        walk_graph_step(self, step);
    }

    /// A syntactic type expression (`array<record<user>>`).
    fn visit_type_expr(&mut self, ty: &Spanned<TypeExpr>) {
        walk_type_expr(self, ty);
    }
}

// --- Row-table helpers ------------------------------------------------------

/// The table whose rows a statement's clauses evaluate against, when its
/// sources/targets name exactly one plain table or record id. Any other shape
/// — none, several, a subquery, a parameter — leaves the row table unknown.
pub fn row_table(sources: &[Spanned<Expr>]) -> Option<&str> {
    match sources {
        [only] => expr_table_name(&only.node),
        _ => None,
    }
}

/// The table a source/target expression names, if it is a bare table or a
/// record id (`person` / `person:one`).
pub fn expr_table_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Table(name) => Some(name.node.as_str()),
        Expr::RecordId { table, .. } => Some(table.node.as_str()),
        _ => None,
    }
}

/// The edge table a graph step's inline `WHERE` evaluates against: its single
/// named target. A wildcard step (`->?`) or a multi-target step names none.
fn step_table(step: &GraphStep) -> Option<&str> {
    match (step.wildcard, step.targets.as_slice()) {
        (false, [only]) => Some(only.node.as_str()),
        _ => None,
    }
}

// --- Statements -------------------------------------------------------------

/// Visits each statement of a lowered source file.
pub fn walk_script<V: Visitor>(visitor: &mut V, script: &Script) {
    let Script { statements } = script;
    for statement in statements {
        visitor.visit_statement(statement);
    }
}

/// Dispatches a statement to its per-kind `visit_*` method.
pub fn walk_statement<V: Visitor>(visitor: &mut V, statement: &Spanned<Statement>) {
    match &statement.node {
        Statement::Select(select) => visitor.visit_select(select),
        Statement::Create(create) => visitor.visit_create(create),
        Statement::Update(update) => visitor.visit_update(update),
        Statement::Upsert(upsert) => visitor.visit_upsert(upsert),
        Statement::Delete(delete) => visitor.visit_delete(delete),
        Statement::Insert(insert) => visitor.visit_insert(insert),
        Statement::Relate(relate) => visitor.visit_relate(relate),
        Statement::Define(define) => visitor.visit_define(define),
        Statement::Remove(remove) => visitor.visit_remove(remove),
        Statement::Alter(alter) => visitor.visit_alter(alter),
        Statement::Let(let_stmt) => visitor.visit_let(let_stmt),
        Statement::Return(ret) => visitor.visit_return(ret),
        Statement::IfElse(if_else) => visitor.visit_if_else(if_else),
        Statement::For(for_stmt) => visitor.visit_for(for_stmt),
        Statement::Block(block) => visitor.visit_block(block),
        Statement::LiveSelect(live) => visitor.visit_live_select(live),
        Statement::Kill(kill) => visitor.visit_kill(kill),
        Statement::Use(use_stmt) => visitor.visit_use(use_stmt),
        Statement::Info(info) => visitor.visit_info(info),
        Statement::Show(show) => visitor.visit_show(show),
        Statement::Rebuild(rebuild) => visitor.visit_rebuild(rebuild),
        Statement::Throw(throw) => visitor.visit_throw(throw),
        Statement::Break(break_stmt) => visitor.visit_break(break_stmt),
        Statement::Continue(continue_stmt) => visitor.visit_continue(continue_stmt),
        Statement::Begin(begin) => visitor.visit_begin(begin),
        Statement::Cancel(cancel) => visitor.visit_cancel(cancel),
        Statement::Commit(commit) => visitor.visit_commit(commit),
        Statement::Sleep(sleep) => visitor.visit_sleep(sleep),
        Statement::Option(option) => visitor.visit_option(option),
        Statement::Expr(expr) => visitor.visit_expr(expr),
        Statement::Partial(partial) => visitor.visit_partial(partial),
    }
}

/// Visits each statement of a block.
pub fn walk_block<V: Visitor>(visitor: &mut V, block: &Block) {
    let Block { statements } = block;
    for statement in statements {
        visitor.visit_statement(statement);
    }
}

/// Visits a SELECT: its sources, then — in the row scope of the single source
/// table — projections, `OMIT`, `WHERE`, `SPLIT`, `GROUP`, `ORDER` and
/// `FETCH`; then the once-per-statement `LIMIT`/`START`/`TIMEOUT`.
pub fn walk_select<V: Visitor>(visitor: &mut V, select: &SelectStmt) {
    let SelectStmt {
        only: _,
        value: _,
        projections,
        from,
        omit,
        fetch,
        split,
        where_clause,
        group,
        order,
        limit,
        start,
        explain: _,
        timeout,
        parallel: _,
    } = select;
    for source in from {
        visitor.visit_expr(source);
    }
    visitor.visit_row_scope(row_table(from), |visitor| {
        for projection in projections {
            visitor.visit_projection(projection);
        }
        for idiom in omit {
            visitor.visit_idiom(&idiom.node);
        }
        if let Some(where_clause) = where_clause {
            visitor.visit_expr(where_clause);
        }
        for idiom in split {
            visitor.visit_idiom(&idiom.node);
        }
        if let Some(group) = group {
            visitor.visit_group_clause(group);
        }
        if let Some(order) = order {
            visitor.visit_order_clause(order);
        }
        for idiom in fetch {
            visitor.visit_idiom(&idiom.node);
        }
    });
    for extra in [limit, start, timeout].into_iter().flatten() {
        visitor.visit_expr(extra);
    }
}

/// Visits a CREATE: its targets, then its payload and `RETURN` in the row
/// scope of the single target table.
pub fn walk_create<V: Visitor>(visitor: &mut V, create: &CreateStmt) {
    let CreateStmt {
        only: _,
        targets,
        data,
        ret,
    } = create;
    for target in targets {
        visitor.visit_expr(target);
    }
    visitor.visit_row_scope(row_table(targets), |visitor| {
        if let Some(data) = data {
            visitor.visit_data_clause(data);
        }
        if let Some(ret) = ret {
            visitor.visit_return_mode(ret);
        }
    });
}

/// Visits an UPDATE: its targets, then payload, `WHERE` and `RETURN` in the
/// row scope of the single target table.
pub fn walk_update<V: Visitor>(visitor: &mut V, update: &UpdateStmt) {
    let UpdateStmt {
        only: _,
        targets,
        data,
        where_clause,
        ret,
    } = update;
    for target in targets {
        visitor.visit_expr(target);
    }
    visitor.visit_row_scope(row_table(targets), |visitor| {
        if let Some(data) = data {
            visitor.visit_data_clause(data);
        }
        if let Some(where_clause) = where_clause {
            visitor.visit_expr(where_clause);
        }
        if let Some(ret) = ret {
            visitor.visit_return_mode(ret);
        }
    });
}

/// Visits an UPSERT: its targets, then payload, `WHERE` and `RETURN` in the
/// row scope of the single target table.
pub fn walk_upsert<V: Visitor>(visitor: &mut V, upsert: &UpsertStmt) {
    let UpsertStmt {
        only: _,
        targets,
        data,
        where_clause,
        ret,
    } = upsert;
    for target in targets {
        visitor.visit_expr(target);
    }
    visitor.visit_row_scope(row_table(targets), |visitor| {
        if let Some(data) = data {
            visitor.visit_data_clause(data);
        }
        if let Some(where_clause) = where_clause {
            visitor.visit_expr(where_clause);
        }
        if let Some(ret) = ret {
            visitor.visit_return_mode(ret);
        }
    });
}

/// Visits a DELETE: its targets, then `WHERE` and `RETURN` in the row scope of
/// the single target table.
pub fn walk_delete<V: Visitor>(visitor: &mut V, delete: &DeleteStmt) {
    let DeleteStmt {
        only: _,
        targets,
        where_clause,
        ret,
    } = delete;
    for target in targets {
        visitor.visit_expr(target);
    }
    visitor.visit_row_scope(row_table(targets), |visitor| {
        if let Some(where_clause) = where_clause {
            visitor.visit_expr(where_clause);
        }
        if let Some(ret) = ret {
            visitor.visit_return_mode(ret);
        }
    });
}

/// Visits an INSERT: its target, then payload, `ON DUPLICATE KEY UPDATE` and
/// `RETURN` in the row scope of the target table.
pub fn walk_insert<V: Visitor>(visitor: &mut V, insert: &InsertStmt) {
    let InsertStmt {
        ignore: _,
        relation: _,
        target,
        data,
        on_duplicate_update,
        ret,
    } = insert;
    if let Some(target) = target {
        visitor.visit_expr(target);
    }
    let table = target
        .as_ref()
        .and_then(|target| expr_table_name(&target.node));
    visitor.visit_row_scope(table, |visitor| {
        visitor.visit_insert_data(data);
        for assignment in on_duplicate_update {
            visitor.visit_assignment(assignment);
        }
        if let Some(ret) = ret {
            visitor.visit_return_mode(ret);
        }
    });
}

/// Visits an INSERT payload: each value, or each row's columns and values.
pub fn walk_insert_data<V: Visitor>(visitor: &mut V, data: &InsertData) {
    match data {
        InsertData::Values(values) => {
            for value in values {
                visitor.visit_expr(value);
            }
        }
        InsertData::Rows {
            rows,
            misaligned: _,
        } => {
            for row in rows {
                for (column, value) in row {
                    visitor.visit_idiom(&column.node);
                    visitor.visit_expr(value);
                }
            }
        }
        InsertData::Partial(partial) => visitor.visit_partial(partial),
    }
}

/// Visits a RELATE: its `from`, edge and `to` positions, then payload and
/// `RETURN` in the row scope of the edge table.
pub fn walk_relate<V: Visitor>(visitor: &mut V, relate: &RelateStmt) {
    let RelateStmt {
        only: _,
        from,
        edge,
        to,
        data,
        ret,
    } = relate;
    for endpoint in [from, edge, to].into_iter().flatten() {
        visitor.visit_expr(endpoint);
    }
    let table = edge.as_ref().and_then(|edge| expr_table_name(&edge.node));
    visitor.visit_row_scope(table, |visitor| {
        if let Some(data) = data {
            visitor.visit_data_clause(data);
        }
        if let Some(ret) = ret {
            visitor.visit_return_mode(ret);
        }
    });
}

/// Dispatches a DEFINE to its per-kind `visit_*` method.
pub fn walk_define<V: Visitor>(visitor: &mut V, define: &DefineStmt) {
    match define {
        DefineStmt::Table(table) => visitor.visit_define_table(table),
        DefineStmt::Field(field) => visitor.visit_define_field(field),
        DefineStmt::Index(index) => visitor.visit_define_index(index),
        DefineStmt::Event(event) => visitor.visit_define_event(event),
        DefineStmt::Param(param) => visitor.visit_define_param(param),
        DefineStmt::Function(function) => visitor.visit_define_function(function),
        DefineStmt::Analyzer(analyzer) => visitor.visit_define_analyzer(analyzer),
        DefineStmt::Other(partial) => visitor.visit_partial(partial),
    }
}

/// Visits a DEFINE TABLE: its relation spec, then its `PERMISSIONS`
/// predicates in the row scope of the table itself. The table's own name is a
/// declaration and is not a [`Visitor::visit_table_ref`].
pub fn walk_define_table<V: Visitor>(visitor: &mut V, table: &DefineTable) {
    let DefineTable {
        name,
        overwrite: _,
        if_not_exists: _,
        schemafull: _,
        relation,
        drop: _,
        changefeed: _,
        permissions,
    } = table;
    if let Some(relation) = relation {
        visitor.visit_relation_def(relation);
    }
    visitor.visit_row_scope(Some(&name.node), |visitor| {
        for predicate in permissions {
            visitor.visit_expr(predicate);
        }
    });
}

/// Visits a relation spec's `IN`/`OUT` endpoint tables as table references.
pub fn walk_relation_def<V: Visitor>(visitor: &mut V, relation: &RelationDef) {
    let RelationDef {
        in_tables,
        out_tables,
        span: _,
    } = relation;
    for endpoint in in_tables.iter().chain(out_tables) {
        visitor.visit_table_ref(endpoint);
    }
}

/// Visits a DEFINE FIELD: its table (a reference) and declared type, then —
/// in the row scope of that table — the field path and the `DEFAULT`,
/// `VALUE`, `COMPUTED`, `ASSERT` and `PERMISSIONS` expressions.
pub fn walk_define_field<V: Visitor>(visitor: &mut V, field: &DefineField) {
    let DefineField {
        path,
        table,
        ty,
        overwrite: _,
        if_not_exists: _,
        default,
        default_always: _,
        value,
        computed,
        reference: _,
        assert,
        readonly: _,
        permissions,
    } = field;
    visitor.visit_table_ref(table);
    if let Some(ty) = ty {
        visitor.visit_type_expr(ty);
    }
    visitor.visit_row_scope(Some(&table.node), |visitor| {
        visitor.visit_idiom(&path.node);
        for expr in [default, value, computed, assert].into_iter().flatten() {
            visitor.visit_expr(expr);
        }
        for predicate in permissions {
            visitor.visit_expr(predicate);
        }
    });
}

/// Visits a DEFINE INDEX: its table (a reference), then the indexed field
/// paths in that table's row scope.
pub fn walk_define_index<V: Visitor>(visitor: &mut V, index: &DefineIndex) {
    let DefineIndex {
        name: _,
        overwrite: _,
        if_not_exists: _,
        table,
        fields,
        kind: _,
    } = index;
    visitor.visit_table_ref(table);
    visitor.visit_row_scope(Some(&table.node), |visitor| {
        for field in fields {
            visitor.visit_idiom(&field.node);
        }
    });
}

/// Visits a DEFINE EVENT: its table (a reference), then `WHEN` and `THEN` in
/// that table's row scope.
pub fn walk_define_event<V: Visitor>(visitor: &mut V, event: &DefineEvent) {
    let DefineEvent {
        name: _,
        overwrite: _,
        if_not_exists: _,
        table,
        when,
        then,
    } = event;
    visitor.visit_table_ref(table);
    visitor.visit_row_scope(Some(&table.node), |visitor| {
        for expr in [when, then].into_iter().flatten() {
            visitor.visit_expr(expr);
        }
    });
}

/// Visits a DEFINE PARAM's `VALUE`.
pub fn walk_define_param<V: Visitor>(visitor: &mut V, param: &DefineParam) {
    let DefineParam {
        name: _,
        overwrite: _,
        if_not_exists: _,
        value,
    } = param;
    if let Some(value) = value {
        visitor.visit_expr(value);
    }
}

/// Visits a DEFINE FUNCTION: its parameter types, return type, and body.
pub fn walk_define_function<V: Visitor>(visitor: &mut V, function: &DefineFunction) {
    let DefineFunction {
        name: _,
        overwrite: _,
        if_not_exists: _,
        params,
        body,
        return_ty,
    } = function;
    for (_, ty) in params {
        if let Some(ty) = ty {
            visitor.visit_type_expr(ty);
        }
    }
    if let Some(return_ty) = return_ty {
        visitor.visit_type_expr(return_ty);
    }
    if let Some(body) = body {
        visitor.visit_block(body);
    }
}

/// A DEFINE ANALYZER carries only names; nothing to descend into.
pub fn walk_define_analyzer<V: Visitor>(_visitor: &mut V, analyzer: &DefineAnalyzer) {
    let DefineAnalyzer {
        name: _,
        overwrite: _,
        if_not_exists: _,
        tokenizers: _,
        filters: _,
    } = analyzer;
}

/// Visits a REMOVE's target.
pub fn walk_remove<V: Visitor>(visitor: &mut V, remove: &RemoveStmt) {
    let RemoveStmt { target } = remove;
    visitor.visit_remove_target(target);
}

/// Visits the table a REMOVE names (a reference), and for `REMOVE FIELD` the
/// field path in that table's row scope. Function, param and analyzer names
/// are declarations being dropped, not references to anything else.
pub fn walk_remove_target<V: Visitor>(visitor: &mut V, target: &RemoveTarget) {
    match target {
        RemoveTarget::Table(name) => visitor.visit_table_ref(name),
        RemoveTarget::Field { field, table } => {
            visitor.visit_table_ref(table);
            visitor.visit_row_scope(Some(&table.node), |visitor| {
                visitor.visit_idiom(&field.node);
            });
        }
        RemoveTarget::Index { index: _, table } | RemoveTarget::Event { event: _, table } => {
            visitor.visit_table_ref(table);
        }
        RemoveTarget::Function(_) | RemoveTarget::Param(_) | RemoveTarget::Analyzer(_) => {}
        RemoveTarget::Other(partial) => visitor.visit_partial(partial),
    }
}

/// Visits the table an ALTER names (a reference).
pub fn walk_alter<V: Visitor>(visitor: &mut V, alter: &AlterStmt) {
    let AlterStmt {
        table,
        schemafull: _,
        drop: _,
    } = alter;
    if let Some(table) = table {
        visitor.visit_table_ref(table);
    }
}

/// Visits a LET's bound value.
pub fn walk_let<V: Visitor>(visitor: &mut V, let_stmt: &LetStmt) {
    let LetStmt { name: _, value } = let_stmt;
    visitor.visit_expr(value);
}

/// Visits a RETURN's value.
pub fn walk_return<V: Visitor>(visitor: &mut V, ret: &ReturnStmt) {
    let ReturnStmt { value } = ret;
    if let Some(value) = value {
        visitor.visit_expr(value);
    }
}

/// Visits each IF/ELSE IF arm, then the ELSE block.
pub fn walk_if_else<V: Visitor>(visitor: &mut V, if_else: &IfElseStmt) {
    let IfElseStmt {
        branches,
        else_branch,
    } = if_else;
    for branch in branches {
        visitor.visit_if_branch(branch);
    }
    if let Some(else_branch) = else_branch {
        visitor.visit_block(else_branch);
    }
}

/// Visits an IF arm's condition, then its body.
pub fn walk_if_branch<V: Visitor>(visitor: &mut V, branch: &IfBranch) {
    let IfBranch { condition, body } = branch;
    visitor.visit_expr(condition);
    visitor.visit_block(body);
}

/// Visits a FOR's iterable, then its body.
pub fn walk_for<V: Visitor>(visitor: &mut V, for_stmt: &ForStmt) {
    let ForStmt {
        binding: _,
        iterable,
        body,
    } = for_stmt;
    visitor.visit_expr(iterable);
    visitor.visit_block(body);
}

/// Visits a LIVE SELECT: its sources, then projections, `WHERE` and `FETCH`
/// in the row scope of the single source table.
pub fn walk_live_select<V: Visitor>(visitor: &mut V, live: &LiveSelectStmt) {
    let LiveSelectStmt {
        diff: _,
        value: _,
        projections,
        from,
        where_clause,
        fetch,
    } = live;
    for source in from {
        visitor.visit_expr(source);
    }
    visitor.visit_row_scope(row_table(from), |visitor| {
        for projection in projections {
            visitor.visit_projection(projection);
        }
        if let Some(where_clause) = where_clause {
            visitor.visit_expr(where_clause);
        }
        for idiom in fetch {
            visitor.visit_idiom(&idiom.node);
        }
    });
}

/// Visits a KILL's live-query id.
pub fn walk_kill<V: Visitor>(visitor: &mut V, kill: &KillStmt) {
    let KillStmt { id } = kill;
    if let Some(id) = id {
        visitor.visit_expr(id);
    }
}

/// A USE carries only names; nothing to descend into.
pub fn walk_use<V: Visitor>(_visitor: &mut V, use_stmt: &UseStmt) {
    let UseStmt {
        namespace: _,
        database: _,
    } = use_stmt;
}

/// Visits the table an INFO names (a reference).
pub fn walk_info<V: Visitor>(visitor: &mut V, info: &InfoStmt) {
    let InfoStmt { table } = info;
    if let Some(table) = table {
        visitor.visit_table_ref(table);
    }
}

/// Visits the table a SHOW CHANGES names (a reference) and its `SINCE`.
pub fn walk_show<V: Visitor>(visitor: &mut V, show: &ShowStmt) {
    let ShowStmt {
        table,
        since,
        span: _,
    } = show;
    if let Some(table) = table {
        visitor.visit_table_ref(table);
    }
    if let Some(since) = since {
        visitor.visit_expr(since);
    }
}

/// Visits the table a REBUILD INDEX names (a reference).
pub fn walk_rebuild<V: Visitor>(visitor: &mut V, rebuild: &RebuildStmt) {
    let RebuildStmt { index: _, table } = rebuild;
    if let Some(table) = table {
        visitor.visit_table_ref(table);
    }
}

/// Visits a THROW's value.
pub fn walk_throw<V: Visitor>(visitor: &mut V, throw: &ThrowStmt) {
    let ThrowStmt { value } = throw;
    if let Some(value) = value {
        visitor.visit_expr(value);
    }
}

/// `BREAK` has no children.
pub fn walk_break<V: Visitor>(_visitor: &mut V, break_stmt: &BreakStmt) {
    let BreakStmt {} = break_stmt;
}

/// `CONTINUE` has no children.
pub fn walk_continue<V: Visitor>(_visitor: &mut V, continue_stmt: &ContinueStmt) {
    let ContinueStmt {} = continue_stmt;
}

/// `BEGIN` has no children.
pub fn walk_begin<V: Visitor>(_visitor: &mut V, begin: &BeginStmt) {
    let BeginStmt {} = begin;
}

/// `CANCEL` has no children.
pub fn walk_cancel<V: Visitor>(_visitor: &mut V, cancel: &CancelStmt) {
    let CancelStmt {} = cancel;
}

/// `COMMIT` has no children.
pub fn walk_commit<V: Visitor>(_visitor: &mut V, commit: &CommitStmt) {
    let CommitStmt {} = commit;
}

/// Visits a SLEEP's duration.
pub fn walk_sleep<V: Visitor>(visitor: &mut V, sleep: &SleepStmt) {
    let SleepStmt { duration } = sleep;
    if let Some(duration) = duration {
        visitor.visit_expr(duration);
    }
}

/// `OPTION` has no children.
pub fn walk_option<V: Visitor>(_visitor: &mut V, option: &OptionStmt) {
    let OptionStmt {} = option;
}

// --- Clauses ----------------------------------------------------------------

/// Visits a projection's expression, if it has one.
pub fn walk_projection<V: Visitor>(visitor: &mut V, projection: &Projection) {
    match projection {
        Projection::Wildcard(_) => {}
        Projection::Expr { expr, alias: _ } => visitor.visit_expr(expr),
        Projection::Partial(partial) => visitor.visit_partial(partial),
    }
}

/// Visits the projections of a `RETURN <fields>`; the keyword modes have no
/// children.
pub fn walk_return_mode<V: Visitor>(visitor: &mut V, ret: &Spanned<ReturnMode>) {
    match &ret.node {
        ReturnMode::None
        | ReturnMode::Null
        | ReturnMode::Diff
        | ReturnMode::Before
        | ReturnMode::After => {}
        ReturnMode::Fields(projections) => {
            for projection in projections {
                visitor.visit_projection(projection);
            }
        }
    }
}

/// Visits a payload clause's assignments, field paths, or value expression.
pub fn walk_data_clause<V: Visitor>(visitor: &mut V, data: &DataClause) {
    match data {
        DataClause::Set(assignments) => {
            for assignment in assignments {
                visitor.visit_assignment(assignment);
            }
        }
        DataClause::Unset(idioms) => {
            for idiom in idioms {
                visitor.visit_idiom(&idiom.node);
            }
        }
        DataClause::Content(expr)
        | DataClause::Merge(expr)
        | DataClause::Patch(expr)
        | DataClause::Replace(expr)
        | DataClause::Single(expr) => visitor.visit_expr(expr),
        DataClause::Partial(partial) => visitor.visit_partial(partial),
    }
}

/// Visits an assignment's target path, then its value.
pub fn walk_assignment<V: Visitor>(visitor: &mut V, assignment: &Assignment) {
    let Assignment {
        target,
        op: _,
        value,
    } = assignment;
    visitor.visit_idiom(&target.node);
    visitor.visit_expr(value);
}

/// Visits each `ORDER BY` key.
pub fn walk_order_clause<V: Visitor>(visitor: &mut V, order: &OrderClause) {
    let OrderClause { keys } = order;
    for key in keys {
        visitor.visit_order_key(key);
    }
}

/// Visits an `ORDER BY` key's expression.
pub fn walk_order_key<V: Visitor>(visitor: &mut V, key: &OrderKey) {
    let OrderKey {
        expr,
        descending: _,
    } = key;
    visitor.visit_expr(expr);
}

/// Visits each `GROUP BY` key path.
pub fn walk_group_clause<V: Visitor>(visitor: &mut V, group: &GroupClause) {
    let GroupClause { all: _, keys } = group;
    for key in keys {
        visitor.visit_idiom(&key.node);
    }
}

// --- Expressions ------------------------------------------------------------

/// Dispatches an expression on its variant, visiting every child.
pub fn walk_expr<V: Visitor>(visitor: &mut V, expr: &Spanned<Expr>) {
    match &expr.node {
        Expr::Literal(literal) => visitor.visit_literal(literal),
        Expr::Idiom(idiom) => visitor.visit_idiom(idiom),
        Expr::Param(_) | Expr::Constant(_) => {}
        Expr::Table(name) => visitor.visit_table_ref(name),
        Expr::RecordId {
            table,
            id: _,
            range: _,
        } => visitor.visit_table_ref(table),
        Expr::Binary { lhs, op: _, rhs } => {
            visitor.visit_expr(lhs);
            visitor.visit_expr(rhs);
        }
        Expr::Range(range) => visitor.visit_range(range),
        Expr::Prefix { op: _, expr } => visitor.visit_expr(expr),
        Expr::Call(call) => visitor.visit_call(call),
        Expr::Object(entries) => {
            for (_, value) in entries {
                visitor.visit_expr(value);
            }
        }
        Expr::Array(items) => {
            for item in items {
                visitor.visit_expr(item);
            }
        }
        Expr::Subquery(statement) => visitor.visit_statement(statement),
        Expr::Block(block) => visitor.visit_block(block),
        Expr::Cast { ty, expr } => {
            visitor.visit_type_expr(ty);
            visitor.visit_expr(expr);
        }
        Expr::Closure(closure) => visitor.visit_closure(closure),
        Expr::Partial(partial) => visitor.visit_partial(partial),
    }
}

/// A literal has no children; the match stays exhaustive so a literal that
/// grows children is a compile error here.
pub fn walk_literal<V: Visitor>(_visitor: &mut V, literal: &Literal) {
    match literal {
        Literal::Int(_)
        | Literal::Float(_)
        | Literal::Decimal
        | Literal::String(_)
        | Literal::Bool(_)
        | Literal::None
        | Literal::Null
        | Literal::Duration(_)
        | Literal::Datetime(_)
        | Literal::Uuid(_)
        | Literal::Regex(_)
        | Literal::Bytes(_)
        | Literal::File(_)
        | Literal::Point(_, _) => {}
    }
}

/// Visits a range's written bounds.
pub fn walk_range<V: Visitor>(visitor: &mut V, range: &Range) {
    let Range {
        start,
        end,
        start_exclusive: _,
        end_inclusive: _,
    } = range;
    for bound in [start, end].into_iter().flatten() {
        visitor.visit_expr(bound);
    }
}

/// Visits a call's arguments.
pub fn walk_call<V: Visitor>(visitor: &mut V, call: &Call) {
    let Call {
        path: _,
        written: _,
        args,
    } = call;
    for arg in args {
        visitor.visit_expr(arg);
    }
}

/// Visits a closure's parameter types, return type, and body.
pub fn walk_closure<V: Visitor>(visitor: &mut V, closure: &Closure) {
    let Closure {
        params,
        return_ty,
        body,
    } = closure;
    for (_, ty) in params {
        if let Some(ty) = ty {
            visitor.visit_type_expr(ty);
        }
    }
    if let Some(return_ty) = return_ty {
        visitor.visit_type_expr(return_ty);
    }
    visitor.visit_expr(body);
}

/// Visits each part of an idiom, in path order.
pub fn walk_idiom<V: Visitor>(visitor: &mut V, idiom: &Idiom) {
    let Idiom { parts } = idiom;
    for part in parts {
        visitor.visit_idiom_part(part);
    }
}

/// Visits an idiom part's children: a leading value, an index or filter
/// expression, a graph step, destructured sub-paths, method arguments.
pub fn walk_idiom_part<V: Visitor>(visitor: &mut V, part: &Spanned<IdiomPart>) {
    match &part.node {
        IdiomPart::Start(inner) => visitor.visit_expr(inner),
        IdiomPart::Field(_) => {}
        IdiomPart::Index(inner) => visitor.visit_expr(inner),
        IdiomPart::All | IdiomPart::Last => {}
        IdiomPart::Graph { dir: _, step } => visitor.visit_graph_step(step),
        IdiomPart::Destructure(idioms) => {
            for idiom in idioms {
                visitor.visit_idiom(&idiom.node);
            }
        }
        IdiomPart::Where(inner) => visitor.visit_expr(inner),
        IdiomPart::Method { name: _, args } => {
            for arg in args {
                visitor.visit_expr(arg);
            }
        }
        IdiomPart::Recurse { bounded: _ } => {}
        IdiomPart::Optional | IdiomPart::Flatten => {}
        IdiomPart::Partial(partial) => visitor.visit_partial(partial),
    }
}

/// Visits a graph step: its target tables (references), its inline `WHERE` in
/// the row scope of the single edge table, its `LIMIT`/`START`, and any
/// unmodeled target syntax.
pub fn walk_graph_step<V: Visitor>(visitor: &mut V, step: &GraphStep) {
    let GraphStep {
        targets,
        where_clause,
        limit,
        start,
        reference: _,
        wildcard: _,
        alias: _,
        unmodeled,
    } = step;
    for target in targets {
        visitor.visit_table_ref(target);
    }
    if let Some(where_clause) = where_clause {
        visitor.visit_row_scope(step_table(step), |visitor| {
            visitor.visit_expr(where_clause);
        });
    }
    for extra in [limit, start].into_iter().flatten() {
        visitor.visit_expr(extra);
    }
    for partial in unmodeled {
        visitor.visit_partial(partial);
    }
}

/// Visits a type expression's nested types (arguments, union members, the
/// optional's inner type, object property types) and literal types.
pub fn walk_type_expr<V: Visitor>(visitor: &mut V, ty: &Spanned<TypeExpr>) {
    match &ty.node {
        TypeExpr::Name(_) => {}
        TypeExpr::Parameterized { name: _, args } => {
            for arg in args {
                visitor.visit_type_expr(arg);
            }
        }
        TypeExpr::Union(members) => {
            for member in members {
                visitor.visit_type_expr(member);
            }
        }
        TypeExpr::Optional(inner) => visitor.visit_type_expr(inner),
        TypeExpr::Literal(literal) => visitor.visit_literal(literal),
        TypeExpr::Object(properties) => {
            for (_, ty) in properties {
                visitor.visit_type_expr(ty);
            }
        }
        TypeExpr::Partial(partial) => visitor.visit_partial(partial),
    }
}
