//! `Place` — the canonical denotation of a narrowable location.
//!
//! Twelve recognizers across `narrow.rs`, `infer.rs` and `check.rs` each
//! answer some version of "does this expression name a location I can refine?"
//! — `guard_path_of`, `idiom_guard_path`, `row_field_path`,
//! `simple_idiom_path_key`, `type_table_path`, … They agree by accident and
//! differ by accident: two of them are the same shape match with different
//! return types (`GuardPath` vs `String`), and two more are the same shape
//! match keyed on a row field rather than a param.
//!
//! There is one question underneath, and [`place_of`] is the one function that
//! answers it. It is also one of only two places in the analyzer that is
//! *supposed* to look at `ast::Expr` for this purpose; every consumer reads a
//! `Place`.
//!
//! **What is a place.** A location whose kind can be refined — so only
//! *naming* steps count. A step that can compute, filter or fail (`.method()`,
//! `[WHERE …]`, a graph traversal, `[*]`) does not name one location, so an
//! expression containing one denotes no place at all. That is a soundness
//! rule, not a limitation: narrowing `$x.items[WHERE p]` would refine a value
//! that the next read need not produce.

use surrealguard_syntax::ast;

/// A location whose kind can be refined.
///
/// Canonical: two syntactically different expressions that provably denote the
/// same location produce the same `Place`. `($x.f)` and `$x.f` are one place,
/// which is why the parenthesized spelling of a guard cannot silently stop
/// narrowing.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Place {
    /// What the location is rooted in.
    pub(crate) root: PlaceRoot,
    /// The naming steps from the root.
    pub(crate) path: Vec<Step>,
}

/// What a place is rooted in.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum PlaceRoot {
    /// `$x` — a `LET` binding, a function parameter, a `FOR` variable, or a
    /// seeded session/context param. The name carries no `$`.
    Param(String),
    /// A bare field of the row currently in scope: a SELECT's projected row, a
    /// `DEFINE FIELD` clause's subject. The *identity of the row* is carried
    /// by the environment, not by the place — exactly as `row_field_path` did.
    RowField,
}

/// One naming step.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Step {
    /// `.name`
    Field(String),
    /// `[0]`, `[-1]` — a constant index.
    ///
    /// Recognized so that `$x[0].f` is *known* not to be a plain field path
    /// rather than accidentally mistaken for one. Refining an element is sound
    /// only against a fixed-length tuple kind, so no consumer narrows through
    /// one yet; [`Place::field_path`] is what they ask instead.
    Index(i64),
}

impl Place {
    /// A bare param place: `$x`.
    pub(crate) fn param(name: impl Into<String>) -> Self {
        Self {
            root: PlaceRoot::Param(name.into()),
            path: Vec::new(),
        }
    }

    /// A bare field of the row in scope: `v`.
    pub(crate) fn row_field(name: impl Into<String>) -> Self {
        Self {
            root: PlaceRoot::RowField,
            path: vec![Step::Field(name.into())],
        }
    }

    /// The plain field segments of this place, or `None` when any step is not
    /// a field.
    ///
    /// This is the shape every consumer accepts today: a subscript ends the
    /// path, because only the exact written field path is refinable.
    pub(crate) fn field_path(&self) -> Option<Vec<String>> {
        self.path
            .iter()
            .map(|step| match step {
                Step::Field(name) => Some(name.clone()),
                Step::Index(_) => None,
            })
            .collect()
    }

    /// This place extended by one naming step, or `None` when the part names
    /// no location.
    ///
    /// What a *prefix* of an idiom denotes, one part at a time. A walk that
    /// reaches a part which computes, filters or fails stops naming a place,
    /// and everything past it reads its declared kind — which is the same
    /// soundness rule [`place_of`] states, applied incrementally.
    pub(crate) fn stepped(&self, part: &ast::IdiomPart) -> Option<Place> {
        let mut path = self.path.clone();
        path.push(step_of(part)?);
        Some(Place {
            root: self.root.clone(),
            path,
        })
    }

    /// The `param.field.field` lookup key (just `param`, or the joined field
    /// path for a row field).
    ///
    /// This is the wire format `narrowed_path` and `NarrowingAnalysis::path`
    /// already use, reproduced exactly — the LSP reads those strings.
    pub(crate) fn key(&self) -> Option<String> {
        let fields = self.field_path()?;
        Some(match &self.root {
            PlaceRoot::Param(param) if fields.is_empty() => param.clone(),
            PlaceRoot::Param(param) => format!("{param}.{}", fields.join(".")),
            PlaceRoot::RowField => fields.join("."),
        })
    }
}

/// The place an expression denotes, or `None` when it denotes no fixed
/// location.
///
/// Sees through parenthesization (`Expr::Subquery(Statement::Expr(..))`).
/// Grouping parentheses lower to the inner expression today, so that wrapper
/// should never reach here — unwrapping it is a defence against the lowering
/// changing back, and it costs one match arm rather than one bug per
/// recognizer.
///
/// Refuses any step that can compute, filter or fail. A `LET`-alias chain
/// (`LET $y = $x; IF $y = NONE …`) is deliberately *not* followed: resolving
/// one changes which expressions narrow, which is a consumer decision and not
/// a normalization.
pub(crate) fn place_of(expr: &ast::Expr) -> Option<Place> {
    match expr {
        ast::Expr::Param(name) => Some(Place::param(name.clone())),
        ast::Expr::Idiom(idiom) => idiom_place(idiom),
        ast::Expr::Subquery(statement) => match &statement.node {
            ast::Statement::Expr(inner) => place_of(&inner.node),
            _ => None,
        },
        _ => None,
    }
}

/// The place an idiom names: a `$param` start followed by naming steps, or a
/// chain of plain fields rooted in the row.
fn idiom_place(idiom: &ast::Idiom) -> Option<Place> {
    let mut parts = idiom.parts.iter();
    let first = parts.next()?;
    let (root, mut path) = match &first.node {
        ast::IdiomPart::Start(start) => {
            let place = place_of(&start.node)?;
            (place.root, place.path)
        }
        // A bare row field: `email`, `profile.email`.
        ast::IdiomPart::Field(name) => (PlaceRoot::RowField, vec![Step::Field(name.clone())]),
        _ => return None,
    };
    for part in parts {
        path.push(step_of(&part.node)?);
    }
    Some(Place { root, path })
}

/// The naming step a part is, or `None` when it computes, filters or fails.
fn step_of(part: &ast::IdiomPart) -> Option<Step> {
    match part {
        ast::IdiomPart::Field(name) => Some(Step::Field(name.clone())),
        ast::IdiomPart::Index(index) => match &index.node {
            ast::Expr::Literal(ast::Literal::Int(value)) => Some(Step::Index(*value)),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    /// The place named by the expression in `RETURN <source>;`.
    fn place(source: &str) -> Option<Place> {
        let query = format!("RETURN {source};");
        let parsed = parse_source(SourceId::new("place:test"), query.as_str()).expect("parses");
        let statements = surrealguard_syntax::lower::lower_statements(&parsed);
        let ast::Statement::Return(stmt) = &statements.first().expect("one statement").node else {
            panic!("expected a RETURN");
        };
        place_of(&stmt.value.as_ref().expect("a value").node)
    }

    #[test]
    fn a_param_and_a_param_field_path_are_places() {
        assert_eq!(place("$x"), Some(Place::param("x")));
        assert_eq!(
            place("$x.f.g"),
            Some(Place {
                root: PlaceRoot::Param("x".into()),
                path: vec![Step::Field("f".into()), Step::Field("g".into())],
            })
        );
    }

    #[test]
    fn a_bare_field_chain_is_rooted_in_the_row() {
        assert_eq!(
            place("email"),
            Some(Place {
                root: PlaceRoot::RowField,
                path: vec![Step::Field("email".into())],
            })
        );
        assert_eq!(
            place("profile.email").and_then(|p| p.key()),
            Some("profile.email".to_string())
        );
    }

    #[test]
    fn parentheses_denote_the_same_place() {
        // The whole point of a normalization: the spelling changes, the
        // denotation does not.
        assert_eq!(place("($x)"), place("$x"));
        assert_eq!(place("(($x.f))"), place("$x.f"));
    }

    #[test]
    fn a_constant_index_is_a_step_but_not_a_field_path() {
        let indexed = place("$x[0].f").expect("a place");
        assert_eq!(indexed.path, vec![Step::Index(0), Step::Field("f".into())]);
        // Refining an element is sound only against a fixed-length tuple, so
        // every consumer asks for the field path and gets nothing here.
        assert_eq!(indexed.field_path(), None);
        assert_eq!(indexed.key(), None);
    }

    #[test]
    fn a_step_that_can_compute_or_filter_is_not_a_place() {
        for source in [
            "$x.f.len()",
            "$x[WHERE active = true]",
            "$x[*]",
            "$x->edge->other",
            "fn::f($x)",
            "$x[$i]",
        ] {
            assert_eq!(place(source), None, "`{source}` must name no place");
        }
    }

    #[test]
    fn a_place_steps_one_part_at_a_time() {
        // What an idiom prefix denotes: the same answer `place_of` gives for
        // the whole idiom, reached one part at a time — which is what lets a
        // read of `$x.f.g` ask whether `$x.f` was narrowed.
        let base = Place::param("x");
        let field = ast::IdiomPart::Field("f".into());
        assert_eq!(
            base.stepped(&field).and_then(|p| p.key()),
            Some("x.f".into())
        );
        // A part that computes names nothing, so the prefix stops there.
        assert_eq!(
            base.stepped(&ast::IdiomPart::All),
            None,
            "a splat names no single location"
        );
    }

    #[test]
    fn the_key_is_the_existing_wire_format() {
        assert_eq!(place("$x").and_then(|p| p.key()), Some("x".to_string()));
        assert_eq!(
            place("$file.folder").and_then(|p| p.key()),
            Some("file.folder".to_string())
        );
    }
}
