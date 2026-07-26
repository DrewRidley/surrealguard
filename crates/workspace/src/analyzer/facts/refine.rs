//! Layer 3 — the interpreter: what a [`Guard`] proves about the kinds in force.
//!
//! [`guard_of`](super::guard::guard_of) said what a predicate *claims*. This
//! module says what that claim *does to a kind*, and it is the last of the
//! three questions §2.1 of the design separates.
//!
//! Two pieces:
//!
//! * [`Refinement`] — a claim as a **function on kinds**, built out of
//!   [`crate::lattice`]'s `meet` and `subtract`. It is deliberately a function
//!   rather than a resolved kind: the same fact is applied to a param binding
//!   (whose kind the environment holds), to a leaf of a projected row object
//!   (whose kind the SELECT post-pass holds), and later to a closure's element
//!   scope. Only the consumer knows the kind, so only the consumer can apply
//!   the refinement.
//! * [`Guard::facts`] — the interpreter, which composes those functions across
//!   `All`/`Any` into one [`Facts`] map keyed by [`Place`].
//!
//! ## What it refuses to conclude
//!
//! * **`Any` refines a place only when every disjunct refines it.**
//!   `a != NONE OR b != NONE` proves nothing about `a` and nothing about `b`:
//!   either disjunct may be the reason the guard held. This is the one place
//!   where being cleverer is tempting and wrong, and it falls out of the
//!   interpreter rather than being a special case.
//! * **`Unknown` contributes nothing** — an unrecognized conjunct weakens an
//!   `All` not at all and destroys an `Any` entirely.
//! * **A meet that cannot be written is not narrowed.** `KindMeet::Unrepresentable`
//!   leaves the consumer's own kind in place; inventing a narrower name for it
//!   would break the soundness direction the lattice module states.
//! * **A refinement that does not tighten reports nothing.** `apply` returns
//!   `None` rather than an equal kind, so a consumer cannot record a narrowing
//!   that did not happen.

use std::collections::BTreeMap;

use surrealdb_types::{Kind, KindLiteral};

use crate::lattice::{meet, subtract, KindMeet};

use super::guard::{Atom, Collection, Guard, OrdOp};
use super::place::Place;

/// How the interpreter looks up the kind currently in force for a place.
///
/// Implemented by the flow environment (params, `LET`s, narrowed paths) today;
/// by the SELECT row post-pass and a closure's element scope as those consumers
/// arrive. THIS is what lets the row side and the param side be one function —
/// today they are two, because the row consumer has no environment.
pub(crate) trait KindOracle {
    /// The kind in force for `place`, or `None` when the oracle cannot say.
    fn kind_of(&self, place: &Place) -> Option<Kind>;
}

/// An oracle that knows nothing. The honest answer for a consumer with no
/// environment — a `WHERE` clause narrowing projected rows — and never a
/// source of a wrong refinement, only of a missing one.
pub(crate) struct NoOracle;

impl KindOracle for NoOracle {
    fn kind_of(&self, _place: &Place) -> Option<Kind> {
        None
    }
}

/// A claim, as a function on kinds.
///
/// Every variant either meets or subtracts, which is why the six hand-written
/// transforms it replaces cannot drift apart again: they are one pair of
/// lattice operations with different operands.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Refinement {
    /// The value *is* this kind and can be nothing else — a sentinel equality.
    Exactly(Kind),
    /// The value inhabits this kind too, so the answer is the meet.
    Meet(Kind),
    /// Every value this kind covers is ruled out.
    Without(Kind),
    /// The value takes this kind outright: it is an element of a collection of
    /// it, so the element kind is what it is — sharpened by a meet where the
    /// two overlap.
    Takes(Kind),
    /// Every part holds. Applied in sequence, so two claims about one place
    /// compose instead of the second silently replacing the first.
    All(Vec<Refinement>),
    /// At least one part holds, so the answer is their union. Yields nothing
    /// unless *every* part tightens — an alternative that leaves the kind wide
    /// leaves the union wide.
    Any(Vec<Refinement>),
}

impl Refinement {
    /// The kind `kind` refines to, or `None` when this claim does not tighten
    /// it.
    ///
    /// `None` is the same contract `narrow_kind` has always had: "nothing to
    /// record". It covers a refinement that cannot apply, one whose result is
    /// the input, and one whose exact answer has no name.
    pub(crate) fn apply(&self, kind: &Kind) -> Option<Kind> {
        let refined = match self {
            Refinement::Exactly(exact) => exact.clone(),
            Refinement::Meet(other) => match meet(kind, other) {
                KindMeet::Exact(refined) => refined,
                // Proven disjoint, or not writable: either way there is no
                // narrower kind this layer is allowed to name.
                KindMeet::Empty | KindMeet::Unrepresentable => return None,
            },
            Refinement::Without(covered) => subtract(kind, covered)?,
            Refinement::Takes(element) => match meet(kind, element) {
                KindMeet::Exact(refined) => refined,
                // The value is an element of a collection of `element`, so it
                // has that kind whether or not it overlaps what the consumer
                // believed.
                KindMeet::Empty | KindMeet::Unrepresentable => element.clone(),
            },
            Refinement::All(parts) => {
                let mut current = kind.clone();
                for part in parts {
                    if let Some(next) = part.apply(&current) {
                        current = next;
                    }
                }
                current
            }
            Refinement::Any(parts) => {
                let mut alternatives = Vec::with_capacity(parts.len());
                for part in parts {
                    // One alternative that does not tighten leaves the union
                    // as wide as the input.
                    alternatives.push(part.apply(kind)?);
                }
                if alternatives.is_empty() {
                    return None;
                }
                Kind::either(alternatives)
            }
        };
        (refined != *kind).then_some(refined)
    }
}

/// The refinements a guard proves, keyed by place.
///
/// A map rather than a list: a place claimed about twice in one guard
/// (`age > 18 AND age != NONE`) is composed once, rather than applied twice in
/// list order with the second silently overwriting the first.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Facts {
    refined: BTreeMap<Place, Refinement>,
}

impl Facts {
    /// Every `(place, refinement)`, in place order. Bare places sort before
    /// their own field paths, so a consumer applying them in order narrows a
    /// base binding before the paths that read through it — the order the
    /// hand-written effect lists happened to have.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&Place, &Refinement)> {
        self.refined.iter()
    }

    /// Adds a claim about one place, composing it with anything already known.
    fn add(&mut self, place: Place, refinement: Refinement) {
        match self.refined.remove(&place) {
            Some(Refinement::All(mut parts)) => {
                parts.push(refinement);
                self.refined.insert(place, Refinement::All(parts));
            }
            Some(existing) => {
                self.refined
                    .insert(place, Refinement::All(vec![existing, refinement]));
            }
            None => {
                self.refined.insert(place, refinement);
            }
        }
    }
}

impl Guard {
    /// Interpret this guard against the kinds `oracle` holds.
    ///
    /// `All` composes per place; `Any` joins per place and only where *every*
    /// disjunct refines it; `True`, `False` and `Unknown` prove nothing.
    ///
    /// (`False` proving nothing is deliberate. The region is unreachable, so
    /// any refinement of it is vacuous; the consumer that cares about a dead
    /// region reads a verdict, not a refinement.)
    pub(crate) fn facts(&self, oracle: &dyn KindOracle) -> Facts {
        let mut facts = Facts::default();
        self.collect(oracle, &mut facts);
        facts
    }

    fn collect(&self, oracle: &dyn KindOracle, facts: &mut Facts) {
        match self {
            Guard::Atom(atom) => {
                if let Some(refinement) = refinement_of(atom, oracle) {
                    facts.add(atom.place().clone(), refinement);
                }
            }
            Guard::All(parts) => {
                for part in parts {
                    part.collect(oracle, facts);
                }
            }
            Guard::Any(parts) => {
                // A disjunct that can never hold is never the reason the guard
                // held, so it does not weaken the others.
                let live: Vec<Facts> = parts
                    .iter()
                    .filter(|part| !matches!(part, Guard::False))
                    .map(|part| part.facts(oracle))
                    .collect();
                let Some((first, rest)) = live.split_first() else {
                    return;
                };
                for (place, refinement) in first.iter() {
                    let mut alternatives = vec![refinement.clone()];
                    // Only a place EVERY disjunct refines is refined at all.
                    for other in rest {
                        match other.refined.get(place) {
                            Some(refinement) => alternatives.push(refinement.clone()),
                            None => break,
                        }
                    }
                    if alternatives.len() == live.len() {
                        facts.add(place.clone(), Refinement::Any(alternatives));
                    }
                }
            }
            Guard::True | Guard::False | Guard::Unknown => {}
        }
    }
}

/// The refinement an atom proves, or `None` when it proves nothing this layer
/// can express.
fn refinement_of(atom: &Atom, oracle: &dyn KindOracle) -> Option<Refinement> {
    Some(match atom {
        // A sentinel equality pins the value to the sentinel itself.
        Atom::IsNone(_) => Refinement::Exactly(Kind::None),
        Atom::IsNull(_) => Refinement::Exactly(Kind::Null),
        // …and its negation removes that sentinel and ONLY that one. `NULL =
        // NONE` is FALSE on the engine, so a NULL survives `!= NONE`.
        Atom::IsNotNone(_) => Refinement::Without(Kind::None),
        Atom::IsNotNull(_) => Refinement::Without(Kind::Null),
        // Truthiness rules out the two option markers, and the falsy literals
        // where the kind names them individually. It does NOT claim a `string`
        // is non-empty or a `number` non-zero: `Kind` cannot express that, and
        // a union of literals is the only place the answer is exact.
        Atom::Truthy(_) => Refinement::All(vec![
            Refinement::Without(Kind::None),
            Refinement::Without(Kind::Null),
            Refinement::Without(Kind::Literal(KindLiteral::Bool(false))),
            Refinement::Without(Kind::Literal(KindLiteral::Integer(0))),
            Refinement::Without(Kind::Literal(KindLiteral::String(String::new()))),
        ]),
        Atom::Eq(_, value) => Refinement::Meet(value.singleton_kind()?),
        Atom::NotEq(_, value) => Refinement::Without(value.singleton_kind()?),
        // `p > x` rules out both option markers: `NONE` is the lowest value the
        // engine orders and `NULL` the next, so neither can be greater than a
        // non-sentinel operand. `p < x` rules out nothing — `NONE < 65` is
        // TRUE, which is why the row-side recognizer this replaces narrowed on
        // one direction only.
        Atom::Ord(_, OrdOp::Gt | OrdOp::GtEq) => Refinement::All(vec![
            Refinement::Without(Kind::None),
            Refinement::Without(Kind::Null),
        ]),
        Atom::Ord(_, OrdOp::Lt | OrdOp::LtEq) => return None,
        Atom::HasKind(_, kind) => Refinement::Meet(kind.clone()),
        Atom::NotKind(_, kind) => Refinement::Without(kind.clone()),
        Atom::InTables(_, tables) => Refinement::Meet(record_kind(tables)),
        Atom::NotInTables(_, tables) => Refinement::Without(record_kind(tables)),
        Atom::Member(_, collection) => Refinement::Takes(element_kind(collection, oracle)?),
    })
}

fn record_kind(tables: &std::collections::BTreeSet<String>) -> Kind {
    Kind::Record(
        tables
            .iter()
            .map(|table| surrealdb_types::Table::from(table.as_str()))
            .collect(),
    )
}

/// The element kind of a membership atom's collection.
///
/// An `Any` element is not a kind anyone learns anything from, so it yields
/// nothing rather than a vacuous refinement. An `option<array<E>>` still
/// yields `E`: nothing is a member of a `NONE`, so on the branch where the
/// membership held the collection was the array.
fn element_kind(collection: &Collection, oracle: &dyn KindOracle) -> Option<Kind> {
    let element = match collection {
        Collection::Elements(kind) => kind.clone(),
        Collection::Of(place) => elements_of(&oracle.kind_of(place)?)?,
    };
    (element != Kind::Any).then_some(element)
}

fn elements_of(kind: &Kind) -> Option<Kind> {
    match kind {
        Kind::Array(element, _) | Kind::Set(element, _) => Some((**element).clone()),
        Kind::Either(variants) => {
            let elements: Vec<Kind> = variants.iter().filter_map(elements_of).collect();
            (!elements.is_empty()).then(|| Kind::either(elements))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::facts::guard::guard_of;
    use crate::analyzer::facts::place::Place;
    use surrealdb_types::Table;
    use surrealguard_syntax::ast;
    use surrealguard_syntax::parse::parse_source;
    use surrealguard_syntax::source::SourceId;

    /// An oracle over a fixed table of places.
    struct Fixed(Vec<(Place, Kind)>);

    impl KindOracle for Fixed {
        fn kind_of(&self, place: &Place) -> Option<Kind> {
            self.0
                .iter()
                .find(|(candidate, _)| candidate == place)
                .map(|(_, kind)| kind.clone())
        }
    }

    fn cond(source: &str) -> ast::Expr {
        let query = format!("IF {source} {{ RETURN 1; }};");
        let parsed = parse_source(SourceId::new("refine:test"), query.as_str()).expect("parses");
        let statements = surrealguard_syntax::lower::lower_statements(&parsed);
        let ast::Statement::IfElse(stmt) = &statements.first().expect("one statement").node else {
            panic!("expected an IF");
        };
        stmt.branches[0].condition.node.clone()
    }

    /// The kind `$x` refines to under `source` at `polarity`, starting from
    /// `kind`.
    fn refine(source: &str, polarity: bool, kind: Kind) -> Option<Kind> {
        refine_with(source, polarity, kind, Fixed(Vec::new()))
    }

    fn refine_with(source: &str, polarity: bool, kind: Kind, oracle: Fixed) -> Option<Kind> {
        let facts = guard_of(&cond(source), polarity, None).facts(&oracle);
        let (_, refinement) = facts
            .iter()
            .find(|(place, _)| **place == Place::param("x"))?;
        refinement.apply(&kind)
    }

    fn option(kind: Kind) -> Kind {
        Kind::either(vec![Kind::None, kind])
    }

    fn string_literal(value: &str) -> Kind {
        Kind::Literal(KindLiteral::String(value.to_string()))
    }

    #[test]
    fn the_two_sentinels_are_removed_independently() {
        let both = Kind::either(vec![Kind::None, Kind::Null, Kind::String]);
        // `!= NONE` keeps the `null`…
        assert_eq!(
            refine("$x != NONE", true, both.clone()),
            Some(Kind::either(vec![Kind::Null, Kind::String]))
        );
        // …and `!= NULL` keeps the `none`. A recognizer that dropped both here
        // shipped a type the database can violate.
        assert_eq!(
            refine("$x != NULL", true, both.clone()),
            Some(Kind::either(vec![Kind::None, Kind::String]))
        );
        // The conjunction removes both.
        assert_eq!(
            refine("$x != NONE AND $x != NULL", true, both),
            Some(Kind::String)
        );
    }

    #[test]
    fn a_positive_sentinel_pins_the_value_to_it() {
        assert_eq!(
            refine("$x = NONE", true, option(Kind::String)),
            Some(Kind::None)
        );
        // Already exactly `none`: nothing to tighten, so nothing is recorded.
        assert_eq!(refine("$x = NONE", true, Kind::None), None);
    }

    #[test]
    fn a_literal_equality_meets_a_literal_union() {
        let stage = Kind::either(vec![
            string_literal("draft"),
            string_literal("review"),
            string_literal("published"),
        ]);
        assert_eq!(
            refine("$x = 'draft'", true, stage.clone()),
            Some(string_literal("draft"))
        );
        // The negation subtracts.
        assert_eq!(
            refine("$x = 'published'", false, stage.clone()),
            Some(Kind::either(vec![
                string_literal("draft"),
                string_literal("review")
            ]))
        );
        // A plain `string` narrows to the singleton…
        assert_eq!(
            refine("$x = 'draft'", true, Kind::String),
            Some(string_literal("draft"))
        );
        // …and a comparison that can never hold narrows nothing rather than
        // inventing a kind the field cannot take.
        assert_eq!(refine("$x = 'draft'", true, Kind::Int), None);
    }

    #[test]
    fn membership_takes_the_element_kind() {
        // An array literal carries its own element union.
        assert_eq!(
            refine("$x IN ['a', 'b']", true, option(Kind::String)),
            Some(Kind::either(vec![string_literal("a"), string_literal("b")]))
        );
        // A collection place is resolved through the oracle…
        let oracle = Fixed(vec![(
            Place::param("pool"),
            Kind::Array(Box::new(Kind::String), None),
        )]);
        assert_eq!(
            refine_with("$x IN $pool", true, option(Kind::String), oracle),
            Some(Kind::String)
        );
        // …and an unresolvable one proves nothing.
        assert_eq!(refine("$x IN $pool", true, option(Kind::String)), None);
    }

    #[test]
    fn a_table_discriminant_narrows_a_record_union() {
        let union = Kind::Record(vec![Table::from("user"), Table::from("folder")]);
        assert_eq!(
            refine("type::table($x) = 'user'", true, union.clone()),
            Some(Kind::Record(vec![Table::from("user")]))
        );
        assert_eq!(
            refine("type::table($x) != 'user'", true, union.clone()),
            Some(Kind::Record(vec![Table::from("folder")]))
        );
        // A table the union cannot be narrows nothing.
        assert_eq!(refine("type::table($x) = 'other'", true, union), None);
    }

    #[test]
    fn a_disjunction_refines_only_what_every_arm_refines() {
        let stage = Kind::either(vec![
            string_literal("draft"),
            string_literal("review"),
            string_literal("published"),
        ]);
        // Both arms refine `$x`, so the union of the two refinements holds.
        assert_eq!(
            refine("$x = 'draft' OR $x = 'review'", true, stage.clone()),
            Some(Kind::either(vec![
                string_literal("draft"),
                string_literal("review")
            ]))
        );
        // One arm refines `$x` and the other does not: nothing is proven.
        assert_eq!(refine("$x = 'draft' OR $y = 'review'", true, stage), None);
        // An arm about the same place that cannot tighten kills the union too.
        assert_eq!(
            refine("$x != NONE OR $x != NULL", true, option(Kind::String)),
            None
        );
    }

    #[test]
    fn an_unknown_conjunct_weakens_nothing() {
        assert_eq!(
            refine("$x != NONE AND fn::check($x)", true, option(Kind::String)),
            Some(Kind::String)
        );
        // …but an unknown DISJUNCT destroys the disjunction.
        assert_eq!(
            refine("$x != NONE OR fn::check($x)", true, option(Kind::String)),
            None
        );
    }

    #[test]
    fn a_kind_predicate_meets_the_kind_it_proves() {
        let mixed = Kind::either(vec![Kind::None, Kind::String, Kind::Int]);
        assert_eq!(
            refine("type::is_string($x)", true, mixed.clone()),
            Some(Kind::String)
        );
        assert_eq!(
            refine("type::is_string($x)", false, mixed.clone()),
            Some(Kind::either(vec![Kind::None, Kind::Int]))
        );
        // `type::is_record` over an untyped value proves it is a record.
        assert_eq!(
            refine("type::is_record($x)", true, Kind::Any),
            Some(Kind::Record(Vec::<Table>::new()))
        );
        assert_eq!(
            refine("type::is_string($x)", true, mixed),
            Some(Kind::String)
        );
    }

    #[test]
    fn truthiness_removes_the_option_markers_and_the_falsy_literals() {
        assert_eq!(
            refine("$x", true, Kind::either(vec![Kind::None, Kind::String])),
            Some(Kind::String)
        );
        assert_eq!(
            refine(
                "$x",
                true,
                Kind::either(vec![
                    Kind::Literal(KindLiteral::Bool(true)),
                    Kind::Literal(KindLiteral::Bool(false))
                ])
            ),
            Some(Kind::Literal(KindLiteral::Bool(true)))
        );
        // It does NOT claim a `string` is non-empty.
        assert_eq!(refine("$x", true, Kind::String), None);
    }

    #[test]
    fn an_ordering_guard_narrows_in_one_direction_only() {
        let optional = Kind::either(vec![Kind::None, Kind::Null, Kind::Int]);
        assert_eq!(refine("$x > 18", true, optional.clone()), Some(Kind::Int));
        assert_eq!(refine("18 < $x", true, optional.clone()), Some(Kind::Int));
        // `NONE < 65` is TRUE on the engine, so a `<` guard proves nothing.
        assert_eq!(refine("$x < 18", true, optional.clone()), None);
        // …and the negation of a `>` guard is a `<=`, which likewise proves
        // nothing.
        assert_eq!(refine("$x > 18", false, optional), None);
    }

    #[test]
    fn two_claims_about_one_place_compose_rather_than_replace() {
        let facts =
            guard_of(&cond("$x != NONE AND $x != NULL"), true, None).facts(&Fixed(Vec::new()));
        // One entry, not two: the map is keyed by place.
        assert_eq!(facts.iter().count(), 1);
    }
}
