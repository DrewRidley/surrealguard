//! The statement environment: `LET` bindings, param defaults, and param
//! uses, threaded through statements in source order with child scopes for
//! blocks and branches.

use std::collections::{BTreeMap, BTreeSet};

use surrealguard_syntax::span::SourceSpan;

use crate::analysis::ParamInference;
use crate::expression::ExpressionFact;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatementEnv {
    lets: BTreeMap<String, ExpressionFact>,
    /// Names that were already bound when this scope began — a `LET` on
    /// one of these shadows the outer binding.
    inherited: BTreeSet<String>,
    param_defaults: BTreeMap<String, ExpressionFact>,
    params: BTreeMap<String, ParamInference>,
}

impl StatementEnv {
    pub fn fork_child_scope(&self) -> Self {
        Self {
            lets: self.lets.clone(),
            inherited: self.lets.keys().cloned().collect(),
            param_defaults: self.param_defaults.clone(),
            params: BTreeMap::new(),
        }
    }

    /// Whether a `LET` of `name` here would shadow a binding from an
    /// enclosing scope.
    pub fn would_shadow(&self, name: &str) -> bool {
        self.inherited.contains(name)
    }

    pub fn define_let(&mut self, name: String, fact: ExpressionFact) {
        self.lets.insert(name, fact);
    }

    pub fn let_fact(&self, name: &str) -> Option<&ExpressionFact> {
        self.lets.get(name)
    }

    pub fn let_facts(&self) -> &BTreeMap<String, ExpressionFact> {
        &self.lets
    }

    pub fn define_param_default(&mut self, name: String, fact: ExpressionFact) {
        self.param_defaults.insert(name, fact);
    }

    pub fn param_default_fact(&self, name: &str) -> Option<&ExpressionFact> {
        self.param_defaults.get(name)
    }

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

    pub fn merge_param_uses_from(&mut self, child: StatementEnv) {
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

    pub fn into_params(self) -> Vec<ParamInference> {
        self.params.into_values().collect()
    }

    /// Records a typed constraint on a parameter from a checkable use
    /// site, unifying with anything already known. Returns the conflict
    /// pair when the kinds cannot be reconciled — the query is then
    /// unsatisfiable by any value (6001).
    pub fn constrain_param(
        &mut self,
        name: String,
        span: SourceSpan,
        kind: surrealdb_types::Kind,
        domain: Option<crate::analysis::ValueDomain>,
    ) -> Option<(surrealdb_types::Kind, surrealdb_types::Kind)> {
        self.record_param_use(name.clone(), span);
        let entry = self.params.get_mut(&name).expect("recorded above");
        match &entry.kind {
            None => entry.kind = Some(kind),
            Some(existing) => match unify_kinds(existing, &kind) {
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
}
