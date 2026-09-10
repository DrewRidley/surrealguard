//! Shared analyzer context and result contracts.
//!
//! Context owns the world an analyzer needs to check against: schema/catalog
//! facts, source text, diagnostics, and source/span helpers. Individual
//! analyzers should return only what their construct evaluates to.

use surrealguard_diagnostics::{Finding, FindingCode, Severity};
use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::SourceSpan;

use crate::config::TargetVersion;
use crate::expression::ExpressionFact;
use crate::schema::{SchemaIndex, TableDef};
use crate::statement_env::StatementEnv;

/// Shared state passed through statement, expression, and function analyzers.
pub struct AnalysisContext<'a> {
    schema: &'a SchemaIndex,
    /// Every `DEFINE` in the workspace, order-independently (the
    /// `GlobalCatalog` pre-pass). [`schema`](Self::schema) is the *incremental*
    /// catalog — what is defined at this point in the walk — which is the right
    /// basis for the ordering contracts (duplicate definition, REMOVE,
    /// read-before-LET). Whether a *reference target* exists at all is not an
    /// ordering contract: SurrealQL applies a workspace as one unit, so a
    /// `record<bb>` is satisfied by a `DEFINE TABLE bb` written anywhere,
    /// before or after. Existence checks consult this; `None` (unit tests and
    /// single-statement entry points, which have no workspace) falls back to
    /// the incremental catalog alone.
    workspace_catalog: Option<&'a SchemaIndex>,
    source: SourceId,
    source_text: &'a str,
    diagnostics: &'a mut Vec<Finding>,
    env: StatementEnv,
    row_table: Option<&'a TableDef>,
    loop_depth: u32,
    /// The end offset of the statement sequence being analyzed — the point a
    /// fall-through narrowing established here stops holding. `None` means the
    /// source's top level, whose sequence ends with the source text.
    scope_end: Option<u32>,
    /// Whether the value produced at this position is read for its
    /// *cardinality* — its emptiness or its length — and never as a scalar
    /// total. `array::is_empty(<here>)` sets it; the position it names is the
    /// one expression directly beneath, so the constructs that consume it
    /// clear it before descending any further.
    cardinality_position: bool,
    /// The SurrealDB release the workspace deploys against
    /// (`analysis.surrealdb_version`), when configured. The 8xxx checks gate
    /// on it; `None` is "the latest", which gates nothing.
    target_version: Option<TargetVersion>,
}

impl<'a> AnalysisContext<'a> {
    /// A top-level context with an empty environment and no row table — the
    /// starting point for analyzing a statement that opens its own scope.
    pub fn new(
        schema: &'a SchemaIndex,
        source: SourceId,
        source_text: &'a str,
        diagnostics: &'a mut Vec<Finding>,
    ) -> Self {
        Self {
            schema,
            workspace_catalog: None,
            source,
            source_text,
            diagnostics,
            env: StatementEnv::default(),
            row_table: None,
            loop_depth: 0,
            scope_end: None,
            cardinality_position: false,
            target_version: None,
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
            workspace_catalog: None,
            source,
            source_text,
            diagnostics,
            env,
            row_table,
            loop_depth: 0,
            scope_end: None,
            cardinality_position: false,
            target_version: None,
        }
    }

    /// Attaches the configured target SurrealDB version (see
    /// [`target_version`](Self::target_version)).
    pub(crate) fn with_target_version(mut self, version: Option<TargetVersion>) -> Self {
        self.target_version = version;
        self
    }

    /// The configured target SurrealDB version, or `None` for "the latest".
    pub fn target_version(&self) -> Option<TargetVersion> {
        self.target_version
    }

    /// Attaches the order-independent workspace catalog (see
    /// [`workspace_catalog`](Self::workspace_catalog)). The pipeline sets it on
    /// every context it builds; entry points with no workspace leave it unset.
    pub(crate) fn with_workspace_catalog(mut self, catalog: &'a SchemaIndex) -> Self {
        self.workspace_catalog = Some(catalog);
        self
    }

    /// Every table name known anywhere in the workspace, for "did you mean"
    /// suggestions on a failed existence check.
    pub(crate) fn known_table_names(&self) -> Vec<&'a str> {
        let mut names: Vec<&'a str> = self.schema.tables.keys().map(String::as_str).collect();
        if let Some(catalog) = self.workspace_catalog {
            names.extend(catalog.tables.keys().map(String::as_str));
        }
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Whether `name` is defined as a table *anywhere* in the workspace —
    /// the order-independent existence question a reference target asks.
    /// Falls back to the incremental catalog when no workspace catalog is
    /// attached.
    pub(crate) fn table_defined_anywhere(&self, name: &str) -> bool {
        self.schema.table(name).is_some()
            || self
                .workspace_catalog
                .is_some_and(|catalog| catalog.table(name).is_some())
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

    /// Whether the value produced here is read only for its cardinality —
    /// `array::is_empty(<here>)`, `array::len(<here>)`, `count(<here>)`.
    ///
    /// The distinction matters to exactly one contract: an ungrouped bare
    /// `count()` is a footgun *as a total* (`N` rows of `{count: 1}`, never one
    /// row of `{count: N}`), and `GROUP ALL` is the remedy. In a cardinality
    /// position the two are **not** interchangeable — with no matching rows the
    /// ungrouped form yields `[]` and the grouped form yields `[{count: 0}]` —
    /// so the same advice inverts the caller's test.
    pub(crate) fn in_cardinality_position(&self) -> bool {
        self.cardinality_position
    }

    /// Runs `f` with the cardinality-position marker set to `reads_cardinality`.
    ///
    /// Scoped rather than sticky: the marker describes one position, so the
    /// construct that occupies it reads the marker and then clears it for
    /// everything nested inside — a `count()` in a *clause* of the consumed
    /// SELECT is in no such position and must keep reporting.
    pub(crate) fn with_cardinality_position<T>(
        &mut self,
        reads_cardinality: bool,
        f: impl FnOnce(&mut AnalysisContext<'a>) -> T,
    ) -> T {
        let previous = self.cardinality_position;
        self.cardinality_position = reads_cardinality;
        let result = f(self);
        self.cardinality_position = previous;
        result
    }

    /// Runs `f` with the loop depth incremented (a `FOR` body).
    pub fn with_loop<T>(&mut self, f: impl FnOnce(&mut AnalysisContext<'_>) -> T) -> T {
        self.loop_depth += 1;
        let result = f(self);
        self.loop_depth -= 1;
        result
    }

    /// The schema/catalog facts every analyzer checks its construct against.
    pub fn schema(&self) -> &'a SchemaIndex {
        self.schema
    }

    /// The source the statement under analysis belongs to, for span construction.
    pub fn source(&self) -> &SourceId {
        &self.source
    }

    /// The full text of the current source, for slicing spans back to code.
    pub fn source_text(&self) -> &'a str {
        self.source_text
    }

    /// The findings accumulated so far this run.
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

    /// Emits an error-severity finding at `span` — a convenience over
    /// building the [`Finding`] and calling [`Self::emit`].
    pub fn emit_error(&mut self, span: SourceSpan, code: FindingCode, message: impl Into<String>) {
        self.emit(Finding::new(span, code, Severity::Error, message));
    }

    /// The environment holding this scope's `LET` bindings and parameter uses.
    pub fn env(&self) -> &StatementEnv {
        &self.env
    }

    /// Binds a `LET` local `name` to `fact` in the current scope.
    pub fn define_local(&mut self, name: String, fact: ExpressionFact) {
        self.env.define_let(name, fact);
    }

    /// Seeds the engine-supplied session params (`$auth`, ...) into the current
    /// scope's env (see [`StatementEnv::seed_session_params`]).
    pub(crate) fn seed_session_params(&mut self) {
        let source = self.source.clone();
        self.env.seed_session_params(&source);
    }

    /// Reverts the engine-supplied session params to unmodeled in the current
    /// scope (see [`StatementEnv::unbind_session_params`]).
    pub(crate) fn unbind_session_params(&mut self) {
        self.env.unbind_session_params();
    }

    /// Records a `LET`/`FOR` binding for editor features (inlay hints, hover,
    /// go-to-def). Read-only w.r.t. diagnostics.
    pub fn record_let_binding(&mut self, binding: crate::analysis::LetBindingAnalysis) {
        self.env.record_let_binding(binding);
    }

    /// Records that a guard narrowed `path` to `kind` over `range`, for editor
    /// features that must answer *per occurrence* (hover, inlay hints) rather
    /// than per binding. Read-only w.r.t. diagnostics.
    pub(crate) fn record_narrowing(
        &mut self,
        path: String,
        range: surrealguard_syntax::span::ByteRange,
        kind: surrealdb_types::Kind,
        by: Option<String>,
    ) {
        let span = SourceSpan::new(self.source.clone(), range);
        self.env
            .record_narrowing(crate::analysis::NarrowingAnalysis {
                path,
                span,
                kind,
                by,
            });
    }

    /// The end offset of the statement sequence being analyzed: a fall-through
    /// narrowing holds from the guard that established it to here. Defaults to
    /// the end of the source (the top-level sequence).
    pub(crate) fn scope_end(&self) -> u32 {
        self.scope_end
            .unwrap_or_else(|| u32::try_from(self.source_text.len()).unwrap_or(u32::MAX))
    }

    /// Runs `f` with the statement sequence ending at `end` — a block or
    /// branch body, whose fall-through narrowings die with it.
    pub(crate) fn with_scope_end<T>(
        &mut self,
        end: Option<u32>,
        f: impl FnOnce(&mut AnalysisContext<'a>) -> T,
    ) -> T {
        let previous = self.scope_end;
        if end.is_some() {
            self.scope_end = end;
        }
        let result = f(self);
        self.scope_end = previous;
        result
    }

    /// Records (or clears) that `binding` holds `type::table($param)`, for
    /// indirect record-discriminant narrowing.
    pub fn set_table_discriminant(&mut self, binding: String, param: Option<String>) {
        self.env.set_table_discriminant(binding, param);
    }

    /// Flow-narrows the idiom path `key` (a `param.field.field` string) to
    /// `kind` in the current scope, so a downstream read of that exact path
    /// resolves to the narrowed kind.
    pub fn define_narrowed_path(&mut self, key: String, kind: surrealdb_types::Kind) {
        self.env.set_narrowed_path(key, kind);
    }

    /// Rebinds the bare param `name` to `fact` as the result of an *active flow
    /// narrowing* (a guard's positive/negative effect), and marks it narrowed
    /// so dead-branch folding may draw a verdict from the tightened kind. Unlike
    /// [`define_local`](Self::define_local) (used for ordinary `LET`s and base
    /// bindings), this records that the tightening came from flow, not from the
    /// declared kind.
    pub fn narrow_local(&mut self, name: String, fact: ExpressionFact) {
        self.env.define_let(name.clone(), fact);
        self.env.mark_param_narrowed(name);
    }

    /// Looks up the fact for `LET` local `name`, or `None` if it is not bound
    /// in scope (in which case `$name` is a host parameter).
    pub fn local(&self, name: &str) -> Option<&ExpressionFact> {
        self.env.let_fact(name)
    }

    /// Records a use of parameter `name` at `span`, feeding read-before-LET
    /// (6004) ordering and host-parameter inference.
    pub fn record_param_use(&mut self, name: String, span: SourceSpan) {
        self.env.record_param_use(name, span);
    }

    /// Records the fact a parameter's `DEFAULT` evaluates to, so later uses
    /// can be checked against it.
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
            self.report_param_conflict(name, span, &existing, &new);
        }
    }

    /// Reports an irreconcilable parameter constraint, at whichever line is
    /// actually wrong.
    ///
    /// A param carrying a `DEFINE PARAM` default is not two uses disagreeing.
    /// The definition **fixed** the value; this position rejects it. So the
    /// finding belongs at the definition, with the use as a related span — the
    /// use is correct SurrealQL, and blaming it was 6001's most confusing
    /// accusation:
    ///
    /// ```text
    /// DEFINE PARAM $lim VALUE 'notanint';
    /// SELECT * FROM t LIMIT $lim;
    ///   error[E6001]: `$lim` cannot satisfy this query:
    ///                 one use needs `string`, this one needs `int`
    ///                 --> the SELECT line, which is correct
    /// ```
    ///
    /// Calling the definition "one use" was the tell. A host param genuinely
    /// has only uses, and for one of those 6001 is exactly right; a defined
    /// param has a value, and a value that cannot inhabit a position it reaches
    /// is [`Position::ParamDefault`]'s contract, checked by the same `decide`
    /// as every other position.
    fn report_param_conflict(
        &mut self,
        name: &str,
        use_span: SourceSpan,
        existing: &surrealdb_types::Kind,
        required: &surrealdb_types::Kind,
    ) {
        if let Some(finding) = self.param_default_violation(name, &use_span, required) {
            self.emit(finding);
            return;
        }
        self.emit(surrealguard_diagnostics::catalog::finding(
            use_span,
            6001,
            format!(
                "`${name}` cannot satisfy this query: one use needs `{existing}`, this one needs `{required}`"
            ),
        ));
    }

    /// The finding a `DEFINE PARAM`'s value raises at a position that rejects
    /// it, or `None` when there is no default or the value is not *proven*
    /// wrong.
    ///
    /// The written constant is what is compared, not the widened kind, which is
    /// why `VALUE 'red'` into a `'red' | 'blue'` position is silent while
    /// `VALUE 'green'` reports — the same recovery every other constant-bearing
    /// position makes.
    fn param_default_violation(
        &self,
        name: &str,
        use_span: &SourceSpan,
        required: &surrealdb_types::Kind,
    ) -> Option<Finding> {
        use crate::analyzer::contract::{Contract, Position};

        let fact = self.env.param_default_fact(name)?;
        let contract = Contract::new(Position::ParamDefault, required.clone());
        let actual = contract.violation(&crate::analyzer::facts::Term::Opaque, fact)?;
        Some(
            surrealguard_diagnostics::catalog::finding(
                fact.span.clone(),
                contract.code(),
                format!(
                    "`${name}`'s value is `{}`, but it is used where a `{}` is required",
                    crate::render::render_offending(&actual, Some(required)),
                    crate::render_kind(required),
                ),
            )
            .with_related(use_span.clone(), format!("`${name}` is used here")),
        )
    }

    /// Records a constraint on `name` derived from a *value comparison*
    /// (`field = $param`, `$param = NONE`, ...). Unlike [`constrain_param`],
    /// this reconciles with comparison semantics — records union, `none` is
    /// compatible with everything — so a param compared against unrelated
    /// record tables is satisfiable and never reports a 6001; only a genuine
    /// scalar clash does. Reserved session params (`$auth`, `$token`, ...) are
    /// externally typed by the runtime, so a comparison against one records the
    /// use but never constrains its kind.
    ///
    /// [`constrain_param`]: Self::constrain_param
    pub fn constrain_param_comparable(
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
        if crate::context_params::is_reserved_session_param(name) {
            // Externally typed by the runtime; a comparison never pins it.
            self.env.record_param_use(name.to_string(), span);
            return;
        }
        if let Some((existing, new)) =
            self.env
                .constrain_param_comparable(name.to_string(), span.clone(), kind, domain)
        {
            self.report_param_conflict(name, span, &existing, &new);
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

    /// Runs `f` in a forked child scope: child `LET` bindings stay local,
    /// while parameter uses and the loop depth propagate back to the parent.
    /// Used for constructs that open a nested block (e.g. an `IF` branch).
    pub fn with_child_env<T>(&mut self, f: impl FnOnce(&mut AnalysisContext<'_>) -> T) -> T {
        let child_env = self.env.fork_child_scope();
        let mut child = AnalysisContext {
            schema: self.schema,
            workspace_catalog: self.workspace_catalog,
            source: self.source.clone(),
            source_text: self.source_text,
            diagnostics: self.diagnostics,
            env: child_env,
            row_table: self.row_table,
            loop_depth: self.loop_depth,
            scope_end: self.scope_end,
            cardinality_position: self.cardinality_position,
            target_version: self.target_version,
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
