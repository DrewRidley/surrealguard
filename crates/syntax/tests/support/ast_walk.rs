//! A recursive span collector over the lowered AST.
//!
//! The robustness tests need one answer to "every span the lowering
//! produced" — statements, expressions, idiom parts, types, clauses — so
//! they can check each against the source text. The walker visits every
//! `Spanned<_>` and bare `ByteRange` the `ast` module exposes, and records
//! every `PartialNode` it meets so a test can say where partiality came
//! from. Exhaustive `match`es (no wildcards) keep it in step with the AST:
//! a new variant fails to compile here until it is walked.

use surrealql_analyzer_syntax::ast::{
    Block, DataClause, DefineStmt, Expr, GraphStep, Idiom, IdiomPart, InsertData, InsertStmt,
    PartialNode, Projection, RemoveTarget, ReturnMode, SelectStmt, Spanned, Statement, TypeExpr,
};
use surrealql_analyzer_syntax::span::ByteRange;

/// Everything the walk found: each span with a label saying what carried it,
/// and each `PartialNode`.
#[derive(Debug, Default)]
pub struct Collected {
    /// `(what, span)` for every span in the AST, in visit order.
    pub spans: Vec<(&'static str, ByteRange)>,
    /// Every `PartialNode` in the AST, in visit order.
    pub partials: Vec<PartialNode>,
}

impl Collected {
    fn span(&mut self, what: &'static str, span: ByteRange) {
        self.spans.push((what, span));
    }

    fn partial(&mut self, what: &'static str, node: &PartialNode) {
        self.span(what, node.span);
        self.partials.push(node.clone());
    }

    fn string(&mut self, what: &'static str, value: &Spanned<String>) {
        self.span(what, value.span);
    }

    fn opt_string(&mut self, what: &'static str, value: Option<&Spanned<String>>) {
        if let Some(value) = value {
            self.string(what, value);
        }
    }

    fn opt_expr(&mut self, value: Option<&Spanned<Expr>>) {
        if let Some(value) = value {
            self.expr(value);
        }
    }

    fn exprs(&mut self, values: &[Spanned<Expr>]) {
        for value in values {
            self.expr(value);
        }
    }

    fn idioms(&mut self, values: &[Spanned<Idiom>]) {
        for value in values {
            self.idiom(value);
        }
    }

    /// Walks every statement of a script or block body.
    pub fn statements(&mut self, statements: &[Spanned<Statement>]) {
        for statement in statements {
            self.statement(statement);
        }
    }

    fn block(&mut self, block: &Block) {
        self.statements(&block.statements);
    }

    fn projections(&mut self, projections: &[Projection]) {
        for projection in projections {
            match projection {
                Projection::Wildcard(span) => self.span("projection wildcard", *span),
                Projection::Expr { expr, alias } => {
                    self.expr(expr);
                    self.opt_string("projection alias", alias.as_ref());
                }
                Projection::Partial(node) => self.partial("projection", node),
            }
        }
    }

    fn return_mode(&mut self, ret: Option<&Spanned<ReturnMode>>) {
        let Some(ret) = ret else { return };
        self.span("return mode", ret.span);
        match &ret.node {
            ReturnMode::None
            | ReturnMode::Null
            | ReturnMode::Diff
            | ReturnMode::Before
            | ReturnMode::After => {}
            ReturnMode::Fields(projections) => self.projections(projections),
        }
    }

    fn data_clause(&mut self, data: Option<&DataClause>) {
        match data {
            None => {}
            Some(DataClause::Set(assignments)) => {
                for assignment in assignments {
                    self.idiom(&assignment.target);
                    self.span("assignment op", assignment.op.span);
                    self.expr(&assignment.value);
                }
            }
            Some(DataClause::Unset(idioms)) => self.idioms(idioms),
            Some(
                DataClause::Content(expr)
                | DataClause::Merge(expr)
                | DataClause::Patch(expr)
                | DataClause::Replace(expr)
                | DataClause::Single(expr),
            ) => self.expr(expr),
            Some(DataClause::Partial(node)) => self.partial("data clause", node),
        }
    }

    /// Walks one statement and everything beneath it.
    pub fn statement(&mut self, statement: &Spanned<Statement>) {
        self.span("statement", statement.span);
        match &statement.node {
            Statement::Select(select) => self.select(select),
            Statement::Create(create) => {
                self.exprs(&create.targets);
                self.data_clause(create.data.as_ref());
                self.return_mode(create.ret.as_ref());
            }
            Statement::Update(update) => {
                self.exprs(&update.targets);
                self.data_clause(update.data.as_ref());
                self.opt_expr(update.where_clause.as_ref());
                self.return_mode(update.ret.as_ref());
            }
            Statement::Upsert(upsert) => {
                self.exprs(&upsert.targets);
                self.data_clause(upsert.data.as_ref());
                self.opt_expr(upsert.where_clause.as_ref());
                self.return_mode(upsert.ret.as_ref());
            }
            Statement::Delete(delete) => {
                self.exprs(&delete.targets);
                self.opt_expr(delete.where_clause.as_ref());
                self.return_mode(delete.ret.as_ref());
            }
            Statement::Insert(insert) => self.insert(insert),
            Statement::Relate(relate) => {
                self.opt_expr(relate.from.as_ref());
                self.opt_expr(relate.edge.as_ref());
                self.opt_expr(relate.to.as_ref());
                self.data_clause(relate.data.as_ref());
                self.return_mode(relate.ret.as_ref());
            }
            Statement::Define(define) => self.define(define),
            Statement::Remove(remove) => match &remove.target {
                RemoveTarget::Table(name) => self.string("remove table", name),
                RemoveTarget::Field { field, table } => {
                    self.idiom(field);
                    self.string("remove field table", table);
                }
                RemoveTarget::Index { index, table } => {
                    self.string("remove index", index);
                    self.string("remove index table", table);
                }
                RemoveTarget::Event { event, table } => {
                    self.string("remove event", event);
                    self.string("remove event table", table);
                }
                RemoveTarget::Function(name) => self.string("remove function", name),
                RemoveTarget::Param(name) => self.string("remove param", name),
                RemoveTarget::Analyzer(name) => self.string("remove analyzer", name),
                RemoveTarget::Other(node) => self.partial("remove target", node),
            },
            Statement::Alter(alter) => self.opt_string("alter table", alter.table.as_ref()),
            Statement::Let(let_stmt) => {
                self.string("let name", &let_stmt.name);
                self.expr(&let_stmt.value);
            }
            Statement::Return(ret) => self.opt_expr(ret.value.as_ref()),
            Statement::IfElse(if_else) => {
                for branch in &if_else.branches {
                    self.expr(&branch.condition);
                    self.block(&branch.body);
                }
                if let Some(else_branch) = &if_else.else_branch {
                    self.block(else_branch);
                }
            }
            Statement::For(for_stmt) => {
                self.string("for binding", &for_stmt.binding);
                self.expr(&for_stmt.iterable);
                self.block(&for_stmt.body);
            }
            Statement::Block(block) => self.block(block),
            Statement::LiveSelect(live) => {
                self.projections(&live.projections);
                self.exprs(&live.from);
                self.opt_expr(live.where_clause.as_ref());
                self.idioms(&live.fetch);
            }
            Statement::Kill(kill) => self.opt_expr(kill.id.as_ref()),
            Statement::Use(use_stmt) => {
                self.opt_string("use namespace", use_stmt.namespace.as_ref());
                self.opt_string("use database", use_stmt.database.as_ref());
            }
            Statement::Info(info) => self.opt_string("info table", info.table.as_ref()),
            Statement::Show(show) => {
                self.opt_string("show table", show.table.as_ref());
                self.opt_expr(show.since.as_ref());
            }
            Statement::Rebuild(rebuild) => {
                self.opt_string("rebuild index", rebuild.index.as_ref());
                self.opt_string("rebuild table", rebuild.table.as_ref());
            }
            Statement::Throw(throw) => self.opt_expr(throw.value.as_ref()),
            Statement::Sleep(sleep) => self.opt_expr(sleep.duration.as_ref()),
            Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Begin(_)
            | Statement::Cancel(_)
            | Statement::Commit(_)
            | Statement::Option(_) => {}
            Statement::Expr(expr) => self.expr(expr),
            Statement::Partial(node) => self.partial("statement", node),
        }
    }

    fn select(&mut self, select: &SelectStmt) {
        self.projections(&select.projections);
        self.exprs(&select.from);
        self.idioms(&select.omit);
        self.idioms(&select.fetch);
        self.idioms(&select.split);
        self.opt_expr(select.where_clause.as_ref());
        if let Some(group) = &select.group {
            self.idioms(&group.keys);
        }
        if let Some(order) = &select.order {
            for key in &order.keys {
                self.expr(&key.expr);
            }
        }
        self.opt_expr(select.limit.as_ref());
        self.opt_expr(select.start.as_ref());
        if let Some(span) = select.explain {
            self.span("explain", span);
        }
        self.opt_expr(select.timeout.as_ref());
        if let Some(span) = select.parallel {
            self.span("parallel", span);
        }
    }

    fn insert(&mut self, insert: &InsertStmt) {
        self.opt_expr(insert.target.as_ref());
        match &insert.data {
            InsertData::Values(values) => self.exprs(values),
            InsertData::Rows { rows, misaligned } => {
                for row in rows {
                    for (column, value) in row {
                        self.idiom(column);
                        self.expr(value);
                    }
                }
                if let Some(misaligned) = misaligned {
                    self.span("insert misaligned", misaligned.span);
                }
            }
            InsertData::Partial(node) => self.partial("insert data", node),
        }
        for assignment in &insert.on_duplicate_update {
            self.idiom(&assignment.target);
            self.span("assignment op", assignment.op.span);
            self.expr(&assignment.value);
        }
        self.return_mode(insert.ret.as_ref());
    }

    fn define(&mut self, define: &DefineStmt) {
        match define {
            DefineStmt::Table(table) => {
                self.string("define table name", &table.name);
                if let Some(relation) = &table.relation {
                    self.span("relation def", relation.span);
                    for name in relation.in_tables.iter().chain(&relation.out_tables) {
                        self.string("relation endpoint", name);
                    }
                }
                self.exprs(&table.permissions);
            }
            DefineStmt::Field(field) => {
                self.idiom(&field.path);
                self.string("define field table", &field.table);
                if let Some(ty) = &field.ty {
                    self.type_expr(ty);
                }
                self.opt_expr(field.default.as_ref());
                self.opt_expr(field.value.as_ref());
                self.opt_expr(field.computed.as_ref());
                self.opt_expr(field.assert.as_ref());
                self.exprs(&field.permissions);
            }
            DefineStmt::Index(index) => {
                self.string("define index name", &index.name);
                self.string("define index table", &index.table);
                self.idioms(&index.fields);
            }
            DefineStmt::Event(event) => {
                self.string("define event name", &event.name);
                self.string("define event table", &event.table);
                self.opt_expr(event.when.as_ref());
                self.opt_expr(event.then.as_ref());
            }
            DefineStmt::Param(param) => {
                self.string("define param name", &param.name);
                self.opt_expr(param.value.as_ref());
            }
            DefineStmt::Function(function) => {
                self.string("define function name", &function.name);
                self.params(&function.params);
                if let Some(body) = &function.body {
                    self.block(body);
                }
                if let Some(ty) = &function.return_ty {
                    self.type_expr(ty);
                }
            }
            DefineStmt::Analyzer(analyzer) => {
                self.string("define analyzer name", &analyzer.name);
                for name in analyzer.tokenizers.iter().chain(&analyzer.filters) {
                    self.string("analyzer stage", name);
                }
            }
            DefineStmt::Other(node) => self.partial("define other", node),
        }
    }

    fn params(&mut self, params: &[(Spanned<String>, Option<Spanned<TypeExpr>>)]) {
        for (name, ty) in params {
            self.string("param name", name);
            if let Some(ty) = ty {
                self.type_expr(ty);
            }
        }
    }

    /// Walks one expression and everything beneath it.
    pub fn expr(&mut self, expr: &Spanned<Expr>) {
        self.span("expr", expr.span);
        match &expr.node {
            Expr::Literal(_) | Expr::Param(_) => {}
            Expr::Idiom(idiom) => self.idiom_parts(idiom),
            Expr::Table(name) => self.string("table", name),
            Expr::Constant(name) => self.string("constant", name),
            Expr::RecordId {
                table,
                id,
                range: _,
            } => {
                self.string("record id table", table);
                self.span("record id id", *id);
            }
            Expr::Binary { lhs, op, rhs } => {
                self.expr(lhs);
                self.span("binary op", op.span);
                self.expr(rhs);
            }
            Expr::Range(range) => {
                if let Some(start) = &range.start {
                    self.expr(start);
                }
                if let Some(end) = &range.end {
                    self.expr(end);
                }
            }
            Expr::Prefix { op, expr } => {
                self.span("prefix op", op.span);
                self.expr(expr);
            }
            Expr::Call(call) => {
                self.string("call path", &call.path);
                self.exprs(&call.args);
            }
            Expr::Object(fields) => {
                for (key, value) in fields {
                    self.string("object key", key);
                    self.expr(value);
                }
            }
            Expr::Array(items) => self.exprs(items),
            Expr::Subquery(statement) => self.statement(statement),
            Expr::Block(block) => self.block(block),
            Expr::Cast { ty, expr } => {
                self.type_expr(ty);
                self.expr(expr);
            }
            Expr::Closure(closure) => {
                self.params(&closure.params);
                if let Some(ty) = &closure.return_ty {
                    self.type_expr(ty);
                }
                self.expr(&closure.body);
            }
            Expr::Partial(node) => self.partial("expr", node),
        }
    }

    fn idiom(&mut self, idiom: &Spanned<Idiom>) {
        self.span("idiom", idiom.span);
        self.idiom_parts(&idiom.node);
    }

    fn idiom_parts(&mut self, idiom: &Idiom) {
        for part in &idiom.parts {
            self.span("idiom part", part.span);
            match &part.node {
                IdiomPart::Start(expr) => self.expr(expr),
                IdiomPart::Field(_)
                | IdiomPart::All
                | IdiomPart::Last
                | IdiomPart::Recurse { .. }
                | IdiomPart::Optional
                | IdiomPart::Flatten => {}
                IdiomPart::Index(expr) | IdiomPart::Where(expr) => self.expr(expr),
                IdiomPart::Graph { dir, step } => {
                    self.span("graph dir", dir.span);
                    self.graph_step(step);
                }
                IdiomPart::Destructure(idioms) => self.idioms(idioms),
                IdiomPart::Method { name, args } => {
                    self.string("method name", name);
                    self.exprs(args);
                }
                IdiomPart::Partial(node) => self.partial("idiom part", node),
            }
        }
    }

    fn graph_step(&mut self, step: &GraphStep) {
        for target in &step.targets {
            self.string("graph target", target);
        }
        if let Some(where_clause) = &step.where_clause {
            self.expr(where_clause);
        }
        if let Some(limit) = &step.limit {
            self.expr(limit);
        }
        if let Some(start) = &step.start {
            self.expr(start);
        }
        self.opt_string("graph alias", step.alias.as_ref());
        for node in &step.unmodeled {
            self.partial("graph unmodeled", node);
        }
    }

    fn type_expr(&mut self, ty: &Spanned<TypeExpr>) {
        self.span("type", ty.span);
        match &ty.node {
            TypeExpr::Name(name) => self.string("type name", name),
            TypeExpr::Parameterized { name, args } => {
                self.string("type constructor", name);
                for arg in args {
                    self.type_expr(arg);
                }
            }
            TypeExpr::Union(variants) => {
                for variant in variants {
                    self.type_expr(variant);
                }
            }
            TypeExpr::Optional(inner) => self.type_expr(inner),
            TypeExpr::Literal(_) => {}
            TypeExpr::Object(properties) => {
                for (key, value) in properties {
                    self.string("type object key", key);
                    self.type_expr(value);
                }
            }
            TypeExpr::Partial(node) => self.partial("type", node),
        }
    }
}

/// Collects every span and `PartialNode` in `statements`.
pub fn collect(statements: &[Spanned<Statement>]) -> Collected {
    let mut collected = Collected::default();
    collected.statements(statements);
    collected
}

/// The span checks every robustness test makes: `start <= end <= len`, both
/// ends on a UTF-8 character boundary. Returns a description of the first
/// violation, so a property test can report it.
pub fn check_span(text: &str, what: &str, span: ByteRange) -> Result<(), String> {
    let (start, end) = (span.start() as usize, span.end() as usize);
    if start > end {
        return Err(format!("{what} span {start}..{end} is reversed"));
    }
    if end > text.len() {
        return Err(format!(
            "{what} span {start}..{end} ends past the text ({} bytes)",
            text.len()
        ));
    }
    if !text.is_char_boundary(start) {
        return Err(format!(
            "{what} span {start}..{end} starts inside a UTF-8 sequence"
        ));
    }
    if !text.is_char_boundary(end) {
        return Err(format!(
            "{what} span {start}..{end} ends inside a UTF-8 sequence"
        ));
    }
    Ok(())
}

/// Whether a `PartialNode` came from parser recovery (an `ERROR` or
/// `MISSING` CST node) rather than from a construct the lowering does not
/// model.
pub fn is_recovery_partial(node: &PartialNode) -> bool {
    node.cst_kind == "ERROR" || node.cst_kind.starts_with("MISSING ")
}
