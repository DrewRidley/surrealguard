//! The statement environment: `LET` bindings, param defaults, and param
//! uses, threaded through statements in source order with child scopes for
//! blocks and branches.

use std::collections::{BTreeMap, BTreeSet};

use surrealguard_syntax::source::SourceId;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use crate::analysis::{LetBindingAnalysis, NarrowingAnalysis, ParamInference};
use crate::expression::{ExpressionFact, ExpressionValueClass};

/// The bindings in scope for a statement: `LET` facts, param defaults, and
/// the accumulated param uses, threaded through statements in source order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatementEnv {
    lets: BTreeMap<String, ExpressionFact>,
    /// Names that were already bound when this scope began — a `LET` on
    /// one of these shadows the outer binding.
    inherited: BTreeSet<String>,
    param_defaults: BTreeMap<String, ExpressionFact>,
    params: BTreeMap<String, ParamInference>,
    /// `LET $t = type::table($x)` records `$t -> $x`, so a later
    /// `IF $t = 'table'` guard narrows `$x` the same as the direct
    /// `type::table($x) = 'table'` form.
    table_discriminants: BTreeMap<String, String>,
    /// Flow-narrowed field paths: a guard like `$file.folder != NONE`
    /// records `"file.folder" -> record<folder>`, so a downstream read of
    /// that exact idiom path resolves to the narrowed kind rather than the
    /// declared `option<record<folder>>`. Keyed on the `param.field.field`
    /// path; only the exact path narrows (never the base param or siblings).
    narrowed_paths: BTreeMap<String, surrealdb_types::Kind>,
    /// Bare params whose binding was tightened by an *active flow narrowing*
    /// in this scope (a prior guard's positive/negative effect), as opposed to
    /// their base declared/seeded binding. Dead-branch folding consults this so
    /// a verdict only ever fires on a subject a guard actually narrowed —
    /// never on an idiomatic defensive check against a declared-non-optional
    /// param. Inherited by child scopes (a branch body sees the outer
    /// narrowing), like [`narrowed_paths`](Self::narrowed_paths).
    narrowed_params: BTreeSet<String>,
    /// Every `LET` binding (and `FOR` loop variable) analysis has seen in
    /// this scope and its already-merged children, in source order. Pure
    /// editor-feature output: recorded as bodies are walked and drained up to
    /// the source's top-level env, never read by any diagnostic.
    let_bindings: Vec<LetBindingAnalysis>,
    /// Every flow narrowing applied in this scope and its already-merged
    /// children, with the region each covers. Pure editor-feature output, on
    /// the same collect-and-drain protocol as
    /// [`let_bindings`](Self::let_bindings): recorded where the refinement is
    /// applied, drained up to the source's top-level env, never read by any
    /// diagnostic (checking sees narrowings through the bindings themselves).
    narrowings: Vec<NarrowingAnalysis>,
}

impl StatementEnv {
    /// A child scope for a block or branch: it inherits the parent's `LET`
    /// bindings and param defaults but collects its own param uses, so a
    /// local `LET` shadows rather than leaks.
    pub fn fork_child_scope(&self) -> Self {
        Self {
            lets: self.lets.clone(),
            inherited: self.lets.keys().cloned().collect(),
            param_defaults: self.param_defaults.clone(),
            params: BTreeMap::new(),
            table_discriminants: self.table_discriminants.clone(),
            narrowed_paths: self.narrowed_paths.clone(),
            narrowed_params: self.narrowed_params.clone(),
            // Child records are collected fresh and drained back on merge, so
            // the parent's already-recorded bindings are not re-emitted.
            let_bindings: Vec::new(),
            narrowings: Vec::new(),
        }
    }

    /// Records a `LET`/`FOR` binding for editor features. Read-only w.r.t.
    /// diagnostics: nothing consults this during checking.
    pub fn record_let_binding(&mut self, binding: LetBindingAnalysis) {
        self.let_bindings.push(binding);
    }

    /// Every `LET`/`FOR` binding recorded in this scope (and merged from
    /// children), in source order.
    pub fn let_bindings(&self) -> &[LetBindingAnalysis] {
        &self.let_bindings
    }

    /// Records that a guard narrowed `path` to `kind` over `span`. Read-only
    /// w.r.t. diagnostics. Exact duplicates are dropped: an expression may be
    /// re-inferred (const folding, closure re-inference at a call site), and
    /// one refinement over one region is one record.
    pub fn record_narrowing(&mut self, narrowing: NarrowingAnalysis) {
        if self.narrowings.contains(&narrowing) {
            return;
        }
        self.narrowings.push(narrowing);
    }

    /// Every flow narrowing recorded in this scope (and merged from children).
    pub fn narrowings(&self) -> &[NarrowingAnalysis] {
        &self.narrowings
    }

    /// The scope's editor-facing facts — its `LET`/`FOR` bindings and its
    /// flow-narrowed regions. Both are collected by the same walk and consumed
    /// together by the pipeline, and neither is read by any diagnostic.
    pub fn editor_facts(&self) -> (Vec<LetBindingAnalysis>, Vec<NarrowingAnalysis>) {
        (self.let_bindings.clone(), self.narrowings.clone())
    }

    /// Records that the idiom path `key` (a `param.field.field` string) is
    /// flow-narrowed to `kind` in this scope. Only the exact path narrows.
    pub fn set_narrowed_path(&mut self, key: String, kind: surrealdb_types::Kind) {
        self.narrowed_paths.insert(key, kind);
    }

    /// The flow-narrowed kind for the idiom path `key`, if a guard proved one.
    pub fn narrowed_path(&self, key: &str) -> Option<&surrealdb_types::Kind> {
        self.narrowed_paths.get(key)
    }

    /// Records that the bare param `name`'s binding was tightened by an active
    /// flow narrowing in this scope (see [`narrowed_params`](Self::narrowed_params)).
    pub fn mark_param_narrowed(&mut self, name: String) {
        self.narrowed_params.insert(name);
    }

    /// Whether the bare param `name` was tightened by an active flow narrowing
    /// in this scope (as opposed to its base declared/seeded binding).
    pub fn is_param_narrowed(&self, name: &str) -> bool {
        self.narrowed_params.contains(name)
    }

    /// Records that `binding` holds `type::table($param)`, so an equality
    /// guard on `binding` narrows `$param`. Passing `None` clears any prior
    /// mapping (a rebind to something else).
    pub fn set_table_discriminant(&mut self, binding: String, param: Option<String>) {
        match param {
            Some(param) => {
                self.table_discriminants.insert(binding, param);
            }
            None => {
                self.table_discriminants.remove(&binding);
            }
        }
    }

    /// The param `binding` is a `type::table(...)` discriminant of, if any.
    pub fn table_discriminant(&self, binding: &str) -> Option<&str> {
        self.table_discriminants.get(binding).map(String::as_str)
    }

    /// Whether a `LET` of `name` here would shadow a binding from an
    /// enclosing scope.
    pub fn would_shadow(&self, name: &str) -> bool {
        self.inherited.contains(name)
    }

    /// Binds `name` to `fact`, shadowing any existing binding.
    pub fn define_let(&mut self, name: String, fact: ExpressionFact) {
        self.lets.insert(name, fact);
    }

    /// Seeds the engine-supplied session/access params (`$auth`, `$token`,
    /// `$session`, `$access`, `$scope`) as bound facts. Called on every
    /// top-level query env and `fn::` body env so these resolve to their
    /// runtime kinds instead of being misclassified as host params, and so a
    /// guard can flow-narrow them. `$auth` seeds as `option<record>`; see
    /// [`crate::context_params::session_context_params`].
    pub fn seed_session_params(&mut self, source: &SourceId) {
        let span = SourceSpan::new(
            source.clone(),
            ByteRange::new(0, 0).expect("empty range is ordered"),
        );
        for (name, kind) in crate::context_params::session_context_params() {
            let mut fact = ExpressionFact::new(span.clone(), ExpressionValueClass::Variable);
            fact.kind = Some(kind);
            self.lets.insert(name, fact);
        }
    }

    /// Removes the engine-supplied session params from this scope, reverting
    /// them to *unmodeled* (a use resolves exactly as before any seeding).
    /// Called at the `DEFINE` boundary: a schema construct establishes its own
    /// context-param bindings — permission predicates bind `$auth` as the
    /// accessed record, function bodies re-seed the session params — so the
    /// top-level `$auth: option<record>` seed must not leak into a
    /// `DEFAULT`/`VALUE`/`THEN` clause and there be treated as a concrete kind
    /// (which would make `DEFAULT $auth` on a `record<T>` field a false 2001).
    pub fn unbind_session_params(&mut self) {
        for name in crate::context_params::session_context_params().keys() {
            self.lets.remove(name);
        }
    }

    /// The fact for the `LET` binding `name`, if in scope.
    pub fn let_fact(&self, name: &str) -> Option<&ExpressionFact> {
        self.lets.get(name)
    }

    /// Every `LET` binding currently in scope.
    pub fn let_facts(&self) -> &BTreeMap<String, ExpressionFact> {
        &self.lets
    }

    /// Records the `DEFINE PARAM` default for `name`, whose kind seeds the
    /// inferred param and makes it optional at the call site.
    pub fn define_param_default(&mut self, name: String, fact: ExpressionFact) {
        self.param_defaults.insert(name, fact);
    }

    /// The declared default fact for param `name`, if any.
    pub fn param_default_fact(&self, name: &str) -> Option<&ExpressionFact> {
        self.param_defaults.get(name)
    }

    /// Notes one use of param `name` at `span`, creating its inference
    /// entry on first sight (seeded from any default) and deduplicating
    /// repeat visits to the same span.
    pub fn record_param_use(&mut self, name: String, span: SourceSpan) {
        let default_kind = self
            .param_default_fact(&name)
            .and_then(|fact| fact.kind.clone());
        let required = default_kind.is_none();
        let key = name.clone();
        self.params
            .entry(name.clone())
            .or_insert_with(|| ParamInference {
                name,
                kind: default_kind,
                domain: None,
                required,
                spans: Vec::new(),
            });
        // Inference and checking may both visit an expression; one use
        // site is one span.
        let spans = &mut self.params.get_mut(&key).expect("just inserted").spans;
        if !spans.contains(&span) {
            spans.push(span);
        }
    }

    /// Folds a child scope's param uses back into this one, unifying kinds
    /// and domains and unioning the `required` flag and spans. Also drains
    /// the child's recorded `LET`/`FOR` bindings and flow narrowings up so the
    /// whole source's editor facts collect at the top-level env.
    pub fn merge_param_uses_from(&mut self, mut child: StatementEnv) {
        self.let_bindings.append(&mut child.let_bindings);
        for narrowing in child.narrowings.drain(..) {
            self.record_narrowing(narrowing);
        }
        for param in child.params.into_values() {
            let entry = self
                .params
                .entry(param.name.clone())
                .or_insert_with(|| ParamInference {
                    name: param.name.clone(),
                    kind: param.kind.clone(),
                    domain: param.domain.clone(),
                    required: param.required,
                    spans: Vec::new(),
                });
            if entry.kind.is_none() {
                entry.kind = param.kind;
            }
            if entry.domain.is_none() {
                entry.domain = param.domain;
            }
            entry.required |= param.required;
            entry.spans.extend(param.spans);
        }
    }

    /// Consumes the scope and returns its collected param inferences.
    pub fn into_params(self) -> Vec<ParamInference> {
        self.params.into_values().collect()
    }

    /// Records a typed constraint on a parameter from a checkable use
    /// site, unifying with anything already known. Returns the conflict
    /// pair when the kinds cannot be reconciled — the query is then
    /// unsatisfiable by any value (6001).
    ///
    /// This is the *exact* form: the param is consumed as `kind` (a function
    /// argument, `SET`, `LIMIT`, ...), so two incompatible demands genuinely
    /// cannot both be met. Value comparisons use
    /// [`constrain_param_comparable`](Self::constrain_param_comparable), whose
    /// reconciliation is looser.
    pub fn constrain_param(
        &mut self,
        name: String,
        span: SourceSpan,
        kind: surrealdb_types::Kind,
        domain: Option<crate::analysis::ValueDomain>,
    ) -> Option<(surrealdb_types::Kind, surrealdb_types::Kind)> {
        self.constrain_with(name, span, kind, domain, unify_kinds)
    }

    /// Records a constraint derived from a *value comparison* (`in = $param`,
    /// `$param = NONE`, ...). SurrealQL comparisons require only comparability,
    /// not that the param BE the other side's kind: records of different tables
    /// compare fine (they just aren't equal), and `NONE` compares against
    /// anything. So this reconciles with [`unify_comparable`] — records union
    /// their tables and `none` is compatible with everything — and only a
    /// genuine scalar clash (int vs string) yields a 6001.
    pub fn constrain_param_comparable(
        &mut self,
        name: String,
        span: SourceSpan,
        kind: surrealdb_types::Kind,
        domain: Option<crate::analysis::ValueDomain>,
    ) -> Option<(surrealdb_types::Kind, surrealdb_types::Kind)> {
        self.constrain_with(name, span, kind, domain, unify_comparable)
    }

    fn constrain_with(
        &mut self,
        name: String,
        span: SourceSpan,
        kind: surrealdb_types::Kind,
        domain: Option<crate::analysis::ValueDomain>,
        unify: impl Fn(&surrealdb_types::Kind, &surrealdb_types::Kind) -> Option<surrealdb_types::Kind>,
    ) -> Option<(surrealdb_types::Kind, surrealdb_types::Kind)> {
        self.record_param_use(name.clone(), span);
        let entry = self.params.get_mut(&name).expect("recorded above");
        match &entry.kind {
            None => entry.kind = Some(kind),
            Some(existing) => match unify(existing, &kind) {
                Some(unified) => entry.kind = Some(unified),
                None => return Some((existing.clone(), kind)),
            },
        }
        if entry.domain.is_none() {
            entry.domain = domain;
        } else if let (
            Some(crate::analysis::ValueDomain::Range { min, max }),
            Some(crate::analysis::ValueDomain::Range {
                min: new_min,
                max: new_max,
            }),
        ) = (&mut entry.domain, &domain)
        {
            // Ranges intersect; enumerable domains keep the first (an
            // intersection refinement can come with a consumer).
            *min = (*min).max(*new_min);
            *max = match (*max, *new_max) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
        }
        None
    }

    /// A snapshot of the collected param inferences without consuming the
    /// scope.
    pub fn params(&self) -> Vec<ParamInference> {
        self.params.values().cloned().collect()
    }
}

/// The kind two constraint sites agree on, when they can: identical kinds,
/// `any` deferring to the specific one, numeric widening picking the
/// narrower, and unions intersecting with their members.
fn unify_kinds(
    a: &surrealdb_types::Kind,
    b: &surrealdb_types::Kind,
) -> Option<surrealdb_types::Kind> {
    use surrealdb_types::Kind;
    if a == b {
        return Some(a.clone());
    }
    match (a, b) {
        (Kind::Any, other) | (other, Kind::Any) => Some(other.clone()),
        (Kind::Number, narrow @ (Kind::Int | Kind::Float | Kind::Decimal))
        | (narrow @ (Kind::Int | Kind::Float | Kind::Decimal), Kind::Number) => {
            Some(narrow.clone())
        }
        (Kind::Either(variants), other) | (other, Kind::Either(variants)) => variants
            .iter()
            .find_map(|variant| unify_kinds(variant, other)),
        _ => None,
    }
}

/// The reconciliation for two constraints that both come from *value
/// comparisons*. Comparisons never make a query unsatisfiable on their own —
/// SurrealQL compares any two values, yielding a boolean rather than an error —
/// so this is deliberately looser than [`unify_kinds`]:
///
/// - `none` is compatible with everything (`$p = NONE` is an existence check,
///   never a demand that `$p` BE none), so it defers to the other kind.
/// - two record kinds union their table sets: a param compared against two
///   different edges' `in`/`out` fields (e.g. `record<account>` and
///   `record<team>`) is satisfiable — it just compares unequal to one of them.
///
/// Everything else falls back to [`unify_kinds`], so a genuine scalar clash
/// (a param compared as an `int` in one place and a `string` in another) still
/// reports a 6001.
fn unify_comparable(
    a: &surrealdb_types::Kind,
    b: &surrealdb_types::Kind,
) -> Option<surrealdb_types::Kind> {
    use surrealdb_types::Kind;
    if a == b {
        return Some(a.clone());
    }
    match (a, b) {
        (Kind::None, other) | (other, Kind::None) => Some(other.clone()),
        (Kind::Record(a_tables), Kind::Record(b_tables)) => {
            let mut tables = a_tables.clone();
            for table in b_tables {
                if !tables.contains(table) {
                    tables.push(table.clone());
                }
            }
            Some(Kind::Record(tables))
        }
        _ => unify_kinds(a, b),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use surrealdb_types::Kind;
    use surrealguard_syntax::source::SourceId;
    use surrealguard_syntax::span::{ByteRange, SourceSpan};

    use super::StatementEnv;
    use crate::expression::{ExpressionFact, ExpressionValueClass};

    fn fact(kind: Kind) -> ExpressionFact {
        ExpressionFact::new(
            SourceSpan::new(SourceId::new("env-test"), ByteRange::new(0, 1).unwrap()),
            ExpressionValueClass::Literal,
        )
        .with_kind(kind.clone())
    }

    #[test]
    fn statement_env_tracks_let_bindings_and_shadowing() {
        let mut env = StatementEnv::default();
        env.define_let("value".into(), fact(Kind::Int));
        env.define_let("value".into(), fact(Kind::String));

        assert_eq!(
            env.let_fact("value").and_then(|fact| fact.kind.clone()),
            Some(Kind::String)
        );
    }

    #[test]
    fn statement_env_child_scopes_see_parent_lets_without_leaking_locals() {
        let mut parent = StatementEnv::default();
        parent.define_let("outer".into(), fact(Kind::Int));

        let mut child = parent.fork_child_scope();
        child.define_let("inner".into(), fact(Kind::String));

        assert_eq!(
            child.let_fact("outer").and_then(|fact| fact.kind.clone()),
            Some(Kind::Int)
        );
        assert_eq!(
            child.let_fact("inner").and_then(|fact| fact.kind.clone()),
            Some(Kind::String)
        );
        assert_eq!(parent.let_fact("inner"), None);
    }

    #[test]
    fn statement_env_collects_external_params_without_duplicate_entries() {
        let mut env = StatementEnv::default();
        let first = SourceSpan::new(SourceId::new("env-test"), ByteRange::new(0, 1).unwrap());
        let second = SourceSpan::new(SourceId::new("env-test"), ByteRange::new(5, 6).unwrap());

        // Inference and checking may both visit an expression: repeat
        // records of the same span are one use; distinct spans accumulate.
        env.record_param_use("name".into(), first.clone());
        env.record_param_use("name".into(), first);
        env.record_param_use("name".into(), second);

        let params: BTreeMap<_, _> = env
            .into_params()
            .into_iter()
            .map(|param| (param.name.clone(), param))
            .collect();
        let name = params.get("name").expect("name param is recorded");
        assert_eq!(name.kind, None);
        assert_eq!(name.spans.len(), 2);
    }

    fn span(start: u32) -> SourceSpan {
        SourceSpan::new(
            SourceId::new("env-test"),
            ByteRange::new(start, start + 1).unwrap(),
        )
    }

    fn record(table: &str) -> Kind {
        Kind::Record(vec![surrealdb_types::Table::from(table)])
    }

    #[test]
    fn comparable_constraint_unions_record_tables_without_conflict() {
        // `WHERE in = $p` (record<account>) then `WHERE out = $p`
        // (record<team>): the param compares against two edges' record fields.
        // Records of different tables compare fine, so this is satisfiable —
        // no 6001 — and the param widens to the union.
        let mut env = StatementEnv::default();
        assert_eq!(
            env.constrain_param_comparable("p".into(), span(0), record("account"), None),
            None
        );
        let conflict = env.constrain_param_comparable("p".into(), span(5), record("team"), None);
        assert_eq!(
            conflict, None,
            "records of different tables do not conflict"
        );

        let kind = env.into_params().into_iter().next().unwrap().kind.unwrap();
        assert_eq!(
            kind,
            Kind::Record(vec![
                surrealdb_types::Table::from("account"),
                surrealdb_types::Table::from("team"),
            ])
        );
    }

    #[test]
    fn comparable_constraint_none_is_compatible_with_records() {
        // `IF $p = NONE` (none) then `WHERE in = $p` (record<account>): the
        // NONE guard is an existence check, not a demand that `$p` be none.
        let mut env = StatementEnv::default();
        assert_eq!(
            env.constrain_param_comparable("p".into(), span(0), Kind::None, None),
            None
        );
        let conflict = env.constrain_param_comparable("p".into(), span(5), record("account"), None);
        assert_eq!(conflict, None, "`none` is comparable with any record");

        let kind = env.into_params().into_iter().next().unwrap().kind.unwrap();
        assert_eq!(kind, record("account"));
    }

    #[test]
    fn comparable_constraint_still_conflicts_on_incompatible_scalars() {
        // A param compared as an `int` in one place and a `string` in another
        // is a genuine irreconcilable use — still a 6001.
        let mut env = StatementEnv::default();
        assert_eq!(
            env.constrain_param_comparable("p".into(), span(0), Kind::Int, None),
            None
        );
        let conflict = env.constrain_param_comparable("p".into(), span(5), Kind::String, None);
        assert_eq!(conflict, Some((Kind::Int, Kind::String)));
    }

    #[test]
    fn exact_constraint_still_conflicts_on_incompatible_records() {
        // The exact form (function argument / SET / LIMIT) keeps pinning: a
        // param passed to two functions wanting different record tables cannot
        // be both — still a conflict.
        let mut env = StatementEnv::default();
        assert_eq!(
            env.constrain_param("p".into(), span(0), Kind::Int, None),
            None
        );
        let conflict = env.constrain_param("p".into(), span(5), Kind::String, None);
        assert_eq!(conflict, Some((Kind::Int, Kind::String)));
    }
}
