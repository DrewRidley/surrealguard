//! Shared analyzer context and result contracts.
//!
//! Context owns the world an analyzer needs to check against: schema/catalog
//! facts, source text, diagnostics, and source/span helpers. Individual
//! analyzers should return only what their construct evaluates to.

use surrealguard_diagnostics::{Finding, FindingCode, Severity};
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::SourceSpan;

use crate::expression::ExpressionFact;
use crate::schema::{SchemaIndex, TableDef};
use crate::statement_env::StatementEnv;

/// Shared state passed through statement, expression, and function analyzers.
pub struct AnalysisContext<'a> {
    schema: &'a SchemaIndex,
    source: SourceId,
    source_text: &'a str,
    diagnostics: &'a mut Vec<Finding>,
    env: StatementEnv,
    row_table: Option<&'a TableDef>,
    loop_depth: u32,
}

impl<'a> AnalysisContext<'a> {
    pub fn new(
        schema: &'a SchemaIndex,
        source: SourceId,
        source_text: &'a str,
        diagnostics: &'a mut Vec<Finding>,
    ) -> Self {
        Self {
            schema,
            source,
            source_text,
            diagnostics,
            env: StatementEnv::default(),
            row_table: None,
            loop_depth: 0,
        }
    }

    /// A context for dispatching analyzers from pure-inference positions:
    /// carries the scope's environment and row table so value-dependent
    /// analyzers behave identically on both paths.
    pub(crate) fn scoped(
        schema: &'a SchemaIndex,
        source: SourceId,
        source_text: &'a str,
        diagnostics: &'a mut Vec<Finding>,
        env: StatementEnv,
        row_table: Option<&'a TableDef>,
    ) -> Self {
        Self {
            schema,
            source,
            source_text,
            diagnostics,
            env,
            row_table,
            loop_depth: 0,
        }
    }

    /// Consumes the context, returning its environment — for callers that
    /// construct one context per statement but thread bindings across them.
    pub(crate) fn into_env(self) -> StatementEnv {
        self.env
    }

    /// Whether the current statement sits inside a `FOR` body.
    pub fn in_loop(&self) -> bool {
        self.loop_depth > 0
    }

    /// Runs `f` with the loop depth incremented (a `FOR` body).
    pub fn with_loop<T>(&mut self, f: impl FnOnce(&mut AnalysisContext<'_>) -> T) -> T {
        self.loop_depth += 1;
        let result = f(self);
        self.loop_depth -= 1;
        result
    }

    pub fn schema(&self) -> &'a SchemaIndex {
        self.schema
    }

    pub fn source(&self) -> &SourceId {
        &self.source
    }

    pub fn source_text(&self) -> &'a str {
        self.source_text
    }

    pub fn diagnostics(&self) -> &[Finding] {
        self.diagnostics
    }

    /// Records a finding, dropping exact duplicates: re-inference of the
    /// same expression (const-value resolution, closure re-inference at a
    /// call site) may re-detect the same violation at the same span.
    pub fn emit(&mut self, finding: Finding) {
        if self.diagnostics.contains(&finding) {
            return;
        }
        self.diagnostics.push(finding);
    }

    pub fn emit_error(&mut self, span: SourceSpan, code: FindingCode, message: impl Into<String>) {
        self.emit(Finding::new(span, code, Severity::Error, message));
    }

    pub fn env(&self) -> &StatementEnv {
        &self.env
    }

    pub fn define_local(&mut self, name: String, fact: ExpressionFact) {
        self.env.define_let(name, fact);
    }

    pub fn local(&self, name: &str) -> Option<&ExpressionFact> {
        self.env.let_fact(name)
    }

    pub fn record_param_use(&mut self, name: String, span: SourceSpan) {
        self.env.record_param_use(name, span);
    }

    pub fn define_param_default(&mut self, name: String, fact: ExpressionFact) {
        self.env.define_param_default(name, fact);
    }

    /// Records a typed constraint on an unbound parameter; irreconcilable
    /// constraints mean no value can satisfy the query (6001).
    pub fn constrain_param(
        &mut self,
        name: &str,
        span: SourceSpan,
        kind: surrealdb_types::Kind,
        domain: Option<crate::analysis::ValueDomain>,
    ) {
        if self.env.let_fact(name).is_some() {
            // Bound locally: not a host parameter.
            return;
        }
        if let Some((existing, new)) =
            self.env
                .constrain_param(name.to_string(), span.clone(), kind, domain)
        {
            self.emit(surrealguard_diagnostics::catalog::finding(
                span,
                6001,
                format!(
                    "`${name}` cannot satisfy this query: one use needs `{existing}`, this one needs `{new}`"
                ),
            ));
        }
    }

    /// The schema table backing the row/document currently in scope, if
    /// any (e.g. the `FROM` target of an enclosing `SELECT`, or the target
    /// table of a mutation). `None` outside any row context, such as a
    /// top-level `RETURN` with no enclosing statement providing a row.
    pub fn row_table(&self) -> Option<&'a TableDef> {
        self.row_table
    }

    /// Runs `f` with the row-table context set to `table` for its duration,
    /// restoring the previous row table afterward. Used when descending
    /// into a construct that establishes its own row context (e.g. a
    /// `SELECT`'s `FROM` target) so nested expression/function analyzers
    /// can resolve bare field paths.
    pub fn with_row_table<T>(
        &mut self,
        table: Option<&'a TableDef>,
        f: impl FnOnce(&mut AnalysisContext<'_>) -> T,
    ) -> T {
        let previous = self.row_table;
        self.row_table = table;
        let result = f(self);
        self.row_table = previous;
        result
    }

    pub fn with_child_env<T>(&mut self, f: impl FnOnce(&mut AnalysisContext<'_>) -> T) -> T {
        let child_env = self.env.fork_child_scope();
        let mut child = AnalysisContext {
            schema: self.schema,
            source: self.source.clone(),
            source_text: self.source_text,
            diagnostics: self.diagnostics,
            env: child_env,
            row_table: self.row_table,
            loop_depth: self.loop_depth,
        };
        let result = f(&mut child);
        self.loop_depth = child.loop_depth;
        self.env.merge_param_uses_from(child.env);
        result
    }
}

#[cfg(test)]
mod tests {
    use surrealdb_types::Kind;
    use surrealguard_diagnostics::Finding;
    use surrealguard_syntax::source::SourceId;
    use surrealguard_syntax::span::{ByteRange, SourceSpan};

    use super::AnalysisContext;
    use crate::expression::{ExpressionFact, ExpressionValueClass};
    use crate::schema::SchemaIndex;

    #[test]
    fn analysis_context_forks_env_for_nested_scope_without_parent_leaks() {
        let schema = SchemaIndex::default();
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            SourceId::new("query:env"),
            "RETURN $outer;",
            &mut diagnostics,
        );

        ctx.define_local("outer".into(), fact(Kind::Int));
        let child_result = ctx.with_child_env(|child| {
            child.define_local("inner".into(), fact(Kind::String));
            assert_eq!(
                child.local("outer").and_then(|fact| fact.kind.clone()),
                Some(Kind::Int)
            );
            assert_eq!(
                child.local("inner").and_then(|fact| fact.kind.clone()),
                Some(Kind::String)
            );
            child.local("inner").and_then(|fact| fact.kind.clone())
        });

        assert_eq!(child_result, Some(Kind::String));
        assert_eq!(
            ctx.local("outer").and_then(|fact| fact.kind.clone()),
            Some(Kind::Int)
        );
        assert_eq!(ctx.local("inner"), None);
    }

    #[test]
    fn child_scope_param_uses_are_merged_back_without_leaking_child_lets() {
        let schema = SchemaIndex::default();
        let mut diagnostics: Vec<Finding> = Vec::new();
        let mut ctx = AnalysisContext::new(
            &schema,
            SourceId::new("query:child-param"),
            "IF $name THEN { LET $inner = 1; };",
            &mut diagnostics,
        );
        let span = SourceSpan::new(
            SourceId::new("query:child-param"),
            ByteRange::new(3, 8).unwrap(),
        );

        ctx.with_child_env(|child| {
            child.define_local("inner".into(), fact(Kind::Int));
            child.record_param_use("name".into(), span.clone());
        });

        assert_eq!(ctx.local("inner"), None);
        let params: Vec<_> = ctx.env().params();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "name");
        assert_eq!(params[0].spans, vec![span]);
    }

    fn fact(kind: Kind) -> ExpressionFact {
        ExpressionFact::new(
            SourceSpan::new(SourceId::new("query:env"), ByteRange::new(0, 1).unwrap()),
            ExpressionValueClass::Literal,
        )
        .with_kind(kind.clone())
    }
}
