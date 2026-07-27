//! One `Kind` renderer, four audiences.
//!
//! A kind is spelled for a reader, and the reader changes what the right
//! spelling is. `option<string>` is right when the text stands in for
//! something the author *wrote* — a `DEFINE FIELD … TYPE option<string>` the
//! reader can go and look at. It is wrong at a narrowed occurrence, where the
//! fact worth showing is *which members survived the guard* and `none | string`
//! says it. It is wrong again in a diagnostic whose whole subject is the
//! `none`, because folding the member away hides the finding's reason.
//!
//! Before this module the spelling was decided by which code path happened to
//! run: [`render_kind`] folded `none` unconditionally, a bare `{kind}`
//! interpolation used `Kind`'s own `Display` and never folded, and the inlay
//! surfaces cut the *string* at 48 characters — producing
//! `option<string | int | datetime | uuid | decim…`, which is not a type. Four
//! spellings nobody chose.
//!
//! So the audience becomes a parameter: [`render(kind, ctx)`](render). Every
//! call site already knows which one it is.
//!
//! [`KindContext::Declared`] is the default and reproduces the previous
//! `render_kind` bytes exactly, which is what lets the golden snapshot and the
//! inference tests that assert through it stay put while the other three
//! contexts move.

use surrealdb_types::{Kind, KindLiteral, Table};

use crate::lattice::kinds_are_disjoint;

/// Why a kind is being shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KindContext<'a> {
    /// The text stands in for something the author wrote. Mirror their
    /// spelling: `option<T>`, `record<a | b>`, `array<T, 3>`. This is the
    /// historical behaviour and the default, so nothing moves until a call
    /// site opts out.
    Declared,
    /// The kind at one occurrence, after flow narrowing. Optionality is
    /// spelled out (`none | string`), because the fact being communicated is
    /// which members are still live *here*, not that the declaration was
    /// optional.
    ///
    /// `proved` is the claim that refined it — "narrowed by `$x != NONE`" —
    /// which the guard IR makes available as a value for the first time. `None`
    /// where the kind is the declared one, or where the narrowing came from the
    /// recognizer path, which cannot say why.
    Occurrence {
        /// The claim that proved the refinement, canonically written.
        proved: Option<&'a str>,
    },
    /// A glanceable label with a hard budget. The ONLY context permitted to
    /// drop information, and it drops it *structurally* — by widening whole
    /// members, object bodies and table sets — so the result is always a
    /// shorter **true** type, never a prefix of a string.
    Glance {
        /// The character budget the label must fit in.
        budget: usize,
    },
    /// A diagnostic message. Never elides.
    ///
    /// `blame` is the contract the value failed — what the position required.
    /// Members of the rendered kind that are *provably disjoint* from it are
    /// the members at fault, and are forced visible rather than folded away:
    /// a 2001 on an `option<string>` value against a `string` field must say
    /// `none | string`, since the `none` is the entire finding. Members
    /// compatible with `blame` may still fold, so an `option<int>` value
    /// against an `option<string>` field stays `option<int>` — there the
    /// `none` is not what is wrong.
    ///
    /// `None` means the position states no kind contract (a method receiver,
    /// an operator with no single expected operand): nothing can be shown to
    /// be irrelevant, so nothing folds.
    Diagnostic {
        /// The kind the position required, when it required one.
        blame: Option<&'a Kind>,
    },
}

/// The rendered text, and — for an occurrence the analysis narrowed — one line
/// of why this and not the declared kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    /// The type text.
    pub text: String,
    /// "narrowed by `$x != NONE`", when the caller supplied the claim.
    pub note: Option<String>,
}

/// Renders `kind` for the audience `ctx` names.
pub fn render(kind: &Kind, ctx: KindContext<'_>) -> Rendered {
    let mut note = None;
    let text = match ctx {
        KindContext::Declared => write_kind(kind, Style::DECLARED),
        KindContext::Occurrence { proved } => {
            note = proved.map(|claim| format!("narrowed by `{claim}`"));
            write_kind(kind, Style::SPELLED_OUT)
        }
        KindContext::Glance { budget } => glance(kind, budget),
        KindContext::Diagnostic { blame } => {
            // The `none` folds into `option<…>` only when it is *not* the
            // member at fault. Absent a contract, nothing is provably
            // irrelevant, so nothing folds.
            let fold = blame.is_some_and(|blame| !kinds_are_disjoint(&Kind::None, blame));
            write_kind(
                kind,
                if fold {
                    Style::DECLARED
                } else {
                    Style::SPELLED_OUT
                },
            )
        }
    };
    Rendered { text, note }
}

/// Renders a [`Kind`] as the author would have written it: `record<file>`,
/// `array<{ name: string }>`, `option<string>`.
///
/// Preserved as `render(kind, Declared).text` because it is this crate's
/// public spelling of a kind and the quality harness compares against it.
pub fn render_kind(kind: &Kind) -> String {
    render(kind, KindContext::Declared).text
}

/// The **offending** side of a diagnostic: the kind a value actually has,
/// where a position could not accept it.
///
/// `blame` is the contract it failed, when the position states one as a kind —
/// the field's declared type, `bool` for a predicate, `int` for a `LIMIT`. It
/// is what decides whether a `none` is the finding's subject (and must be
/// named) or incidental (and may fold); see [`KindContext::Diagnostic`].
///
/// The *declared* side of the same message — the type the reader can go and
/// look at in a `DEFINE` — stays [`render_kind`]. Which of the two a call site
/// wants is never in doubt, and now it is written down.
pub fn render_offending(kind: &Kind, blame: Option<&Kind>) -> String {
    render(kind, KindContext::Diagnostic { blame }).text
}

/// The two spelling decisions the contexts differ on. Kept internal: a caller
/// picks an audience, not a flag.
#[derive(Clone, Copy)]
struct Style {
    /// Fold a union's `none` member into the `option<…>` wrapper.
    fold_option: bool,
}

impl Style {
    /// The author's spelling.
    const DECLARED: Style = Style { fold_option: true };
    /// Every member named.
    const SPELLED_OUT: Style = Style { fold_option: false };
}

/// Renders a [`Kind`] compactly. Falls back to the kind's own `Display` for
/// shapes without a special compact form.
fn write_kind(kind: &Kind, style: Style) -> String {
    match kind {
        Kind::Any => "any".to_string(),
        Kind::None => "none".to_string(),
        Kind::Null => "null".to_string(),
        Kind::Bool => "bool".to_string(),
        Kind::Bytes => "bytes".to_string(),
        Kind::Datetime => "datetime".to_string(),
        Kind::Decimal => "decimal".to_string(),
        Kind::Duration => "duration".to_string(),
        Kind::Float => "float".to_string(),
        Kind::Int => "int".to_string(),
        Kind::Number => "number".to_string(),
        Kind::Object => "object".to_string(),
        Kind::String => "string".to_string(),
        Kind::Uuid => "uuid".to_string(),
        Kind::Regex => "regex".to_string(),
        Kind::Range => "range".to_string(),
        Kind::Record(tables) => wrap_tables("record", tables),
        Kind::Table(tables) => wrap_tables("table", tables),
        Kind::Array(inner, len) => wrap_collection("array", inner, *len, style),
        Kind::Set(inner, len) => wrap_collection("set", inner, *len, style),
        Kind::File(buckets) => {
            if buckets.is_empty() {
                "file".to_string()
            } else {
                format!("file<{}>", buckets.join(", "))
            }
        }
        Kind::Either(variants) => write_either(variants, style),
        Kind::Literal(literal) => write_literal(literal, style),
        // Geometry, Function, and any future variant: defer to Display.
        other => other.to_string(),
    }
}

fn wrap_tables(head: &str, tables: &[Table]) -> String {
    if tables.is_empty() {
        head.to_string()
    } else {
        let names: Vec<&str> = tables.iter().map(Table::as_str).collect();
        format!("{head}<{}>", names.join(" | "))
    }
}

fn wrap_collection(head: &str, inner: &Kind, len: Option<u64>, style: Style) -> String {
    match len {
        Some(len) => format!("{head}<{}, {len}>", write_kind(inner, style)),
        None => format!("{head}<{}>", write_kind(inner, style)),
    }
}

/// `option<T>` when the union is `none` plus other variants; otherwise a
/// `a | b | c` union. A [`Style`] that does not fold spells the `none` out in
/// place, which keeps the member order the analysis produced.
fn write_either(variants: &[Kind], style: Style) -> String {
    if !style.fold_option {
        let spelled: Vec<String> = variants
            .iter()
            .map(|kind| write_kind(kind, style))
            .collect();
        return spelled.join(" | ");
    }
    let has_none = variants.iter().any(|kind| matches!(kind, Kind::None));
    let rest: Vec<String> = variants
        .iter()
        .filter(|kind| !matches!(kind, Kind::None))
        .map(|kind| write_kind(kind, style))
        .collect();
    if rest.is_empty() {
        return "none".to_string();
    }
    let joined = rest.join(" | ");
    if has_none {
        format!("option<{joined}>")
    } else {
        joined
    }
}

fn write_literal(literal: &KindLiteral, style: Style) -> String {
    match literal {
        KindLiteral::String(value) => format!("'{value}'"),
        KindLiteral::Integer(value) => value.to_string(),
        KindLiteral::Float(value) => value.to_string(),
        KindLiteral::Decimal(value) => value.to_string(),
        KindLiteral::Duration(value) => value.to_string(),
        KindLiteral::Bool(value) => value.to_string(),
        KindLiteral::Array(kinds) => {
            let rendered: Vec<String> = kinds.iter().map(|k| write_kind(k, style)).collect();
            format!("[{}]", rendered.join(", "))
        }
        KindLiteral::Object(entries) => {
            let rendered: Vec<String> = entries
                .iter()
                .map(|(name, kind)| format!("{name}: {}", write_kind(kind, style)))
                .collect();
            format!("{{ {} }}", rendered.join(", "))
        }
    }
}

// ---------------------------------------------------------------------------
// Glance: fitting a budget by widening, never by cutting
// ---------------------------------------------------------------------------

/// The label for `kind` within `budget` characters.
///
/// A character cut cannot know that dropping a member would fit and would
/// still be true; it only knows where byte 48 falls, which is how
/// `option<string | int | datetime | uuid | decim…` shipped. So the budget is
/// spent on the *kind* instead: [`widen`] replaces the largest sub-shape with
/// a shorter kind **above** it, repeatedly, until the render fits. Every
/// intermediate is a valid type and admits every value the original does, so
/// the label is always parseable and always true — it just says less.
///
/// A kind that cannot be widened any further is returned over budget rather
/// than cut: `array<datetime>` is 15 characters of irreducible truth.
fn glance(kind: &Kind, budget: usize) -> String {
    let mut current = kind.clone();
    loop {
        let text = write_kind(&current, Style::DECLARED);
        if text.chars().count() <= budget {
            return text;
        }
        match widen(&current) {
            Some(next) => current = next,
            None => return text,
        }
    }
}

/// One step up the lattice: the shortest-spelled kind strictly above `kind`
/// this module knows how to name, or `None` when there is none.
///
/// Composite kinds widen their *contents* first, so a wide object nested in an
/// array loses its body before the array loses its element.
fn widen(kind: &Kind) -> Option<Kind> {
    match kind {
        // A union widens by widening its widest member; when no member can
        // widen, the last one is dropped and `any` stands in for the tail —
        // `option<string | int | any>` is both shorter and still true.
        Kind::Either(variants) => {
            if let Some(widened) = widen_widest(variants) {
                return Some(Kind::Either(widened));
            }
            let last = variants.iter().rposition(|k| !matches!(k, Kind::Any))?;
            if variants.len() < 2 {
                return None;
            }
            let mut kept: Vec<Kind> = variants.to_vec();
            kept.remove(last);
            if !kept.iter().any(|k| matches!(k, Kind::Any)) {
                kept.push(Kind::Any);
            }
            Some(if kept.len() == 1 {
                kept.into_iter().next().expect("one variant")
            } else {
                Kind::Either(kept)
            })
        }
        // An object literal is closed (`kinds::object_is_assignable_to`), so
        // dropping an entry would produce a kind the value does NOT have.
        // The whole body goes instead, to the `object` above it.
        Kind::Literal(KindLiteral::Object(entries)) => {
            let kinds: Vec<Kind> = entries.values().cloned().collect();
            match widen_widest(&kinds) {
                Some(widened) => Some(Kind::Literal(KindLiteral::Object(
                    entries.keys().cloned().zip(widened).collect(),
                ))),
                None => Some(Kind::Object),
            }
        }
        // A literal array is a tuple; its supertype is the unbounded array.
        Kind::Literal(KindLiteral::Array(kinds)) => match widen_widest(kinds) {
            Some(widened) => Some(Kind::Literal(KindLiteral::Array(widened))),
            None => Some(Kind::Array(Box::new(Kind::Any), None)),
        },
        // A scalar literal widens to its base: `'free'` is a `string`.
        Kind::Literal(literal) => crate::kinds::literal_base_kind(&Kind::Literal(literal.clone())),
        Kind::Array(inner, len) => Some(widen_collection(inner, *len, |inner, len| {
            Kind::Array(Box::new(inner), len)
        })?),
        Kind::Set(inner, len) => Some(widen_collection(inner, *len, |inner, len| {
            Kind::Set(Box::new(inner), len)
        })?),
        // `record<a | b>` is below the open `record`.
        Kind::Record(tables) if !tables.is_empty() => Some(Kind::Record(Vec::new())),
        Kind::Table(tables) if !tables.is_empty() => Some(Kind::Table(Vec::new())),
        Kind::File(buckets) if !buckets.is_empty() => Some(Kind::File(Vec::new())),
        _ => None,
    }
}

/// A collection widens its element, then its element to `any`, then loses its
/// length bound — each step a supertype of the last.
fn widen_collection(
    inner: &Kind,
    len: Option<u64>,
    build: impl Fn(Kind, Option<u64>) -> Kind,
) -> Option<Kind> {
    if let Some(widened) = widen(inner) {
        return Some(build(widened, len));
    }
    if !matches!(inner, Kind::Any) {
        return Some(build(Kind::Any, len));
    }
    len.map(|_| build(Kind::Any, None))
}

/// Widens the member with the longest rendering, leaving the rest alone.
/// `None` when no member can widen at all.
fn widen_widest(kinds: &[Kind]) -> Option<Vec<Kind>> {
    let widest = kinds
        .iter()
        .enumerate()
        .filter(|(_, kind)| widen(kind).is_some())
        .max_by_key(|(_, kind)| write_kind(kind, Style::DECLARED).chars().count())
        .map(|(index, _)| index)?;
    let mut widened = kinds.to_vec();
    widened[widest] = widen(&kinds[widest]).expect("the chosen member widens");
    Some(widened)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(entries: &[(&str, Kind)]) -> Kind {
        Kind::Literal(KindLiteral::Object(
            entries
                .iter()
                .map(|(name, kind)| ((*name).to_string(), kind.clone()))
                .collect(),
        ))
    }

    #[test]
    fn render_kind_compact_forms() {
        use surrealdb_types::Table;
        assert_eq!(render_kind(&Kind::String), "string");
        assert_eq!(
            render_kind(&Kind::Record(vec![Table::from("file")])),
            "record<file>"
        );
        assert_eq!(
            render_kind(&Kind::Either(vec![Kind::None, Kind::String])),
            "option<string>"
        );
        assert_eq!(
            render_kind(&Kind::Array(Box::new(Kind::String), None)),
            "array<string>"
        );
        assert_eq!(
            render_kind(&Kind::Either(vec![Kind::Int, Kind::String])),
            "int | string"
        );
        let object = Kind::Literal(KindLiteral::Object(
            [("name".to_string(), Kind::String)].into_iter().collect(),
        ));
        assert_eq!(
            render_kind(&Kind::Array(Box::new(object), None)),
            "array<{ name: string }>"
        );
    }

    #[test]
    fn declared_folds_the_none_into_an_option() {
        let optional = Kind::either(vec![Kind::None, Kind::String]);
        assert_eq!(render_kind(&optional), "option<string>");
        assert_eq!(
            render(&optional, KindContext::Declared).text,
            "option<string>"
        );
    }

    #[test]
    fn an_occurrence_spells_the_surviving_members_out() {
        // The fact worth showing at an occurrence is which members are live
        // here, so the `none` is named rather than folded into a wrapper that
        // reads as "this was declared optional".
        let optional = Kind::either(vec![Kind::None, Kind::String]);
        assert_eq!(render(&optional, KindContext::Occurrence { proved: None }).text, "none | string");
        // Nested optionality is spelled out too — the reader is being told
        // what the value can be, at every depth.
        let row = object(&[("nick", optional)]);
        assert_eq!(
            render(&row, KindContext::Occurrence { proved: None }).text,
            "{ nick: none | string }"
        );
    }

    #[test]
    fn a_diagnostic_forces_the_blamed_member_visible_and_folds_the_rest() {
        let optional = Kind::either(vec![Kind::None, Kind::Int]);
        // Against a `string` contract the `none` is one of the members at
        // fault, so it is named.
        assert_eq!(
            render(
                &optional,
                KindContext::Diagnostic {
                    blame: Some(&Kind::String)
                }
            )
            .text,
            "none | int"
        );
        // Against an `option<string>` contract the `none` is fine — the `int`
        // is the problem — so the spelling stays the compact one.
        let optional_string = Kind::either(vec![Kind::None, Kind::String]);
        assert_eq!(
            render(
                &optional,
                KindContext::Diagnostic {
                    blame: Some(&optional_string)
                }
            )
            .text,
            "option<int>"
        );
        // With no contract to compare against, nothing is provably
        // irrelevant, so nothing folds.
        assert_eq!(
            render(&optional, KindContext::Diagnostic { blame: None }).text,
            "none | int"
        );
    }

    #[test]
    fn a_glance_within_budget_is_the_declared_spelling() {
        let kind = Kind::either(vec![Kind::None, Kind::String]);
        assert_eq!(
            render(&kind, KindContext::Glance { budget: 48 }).text,
            "option<string>"
        );
    }

    #[test]
    fn a_glance_widens_a_wide_object_body_rather_than_cutting_it() {
        let wide = object(
            &(0..12)
                .map(|i| (format!("field_number_{i}"), Kind::String))
                .collect::<Vec<_>>()
                .iter()
                .map(|(name, kind)| (name.as_str(), kind.clone()))
                .collect::<Vec<_>>(),
        );
        let label = render(&wide, KindContext::Glance { budget: 48 }).text;
        // An object literal is closed, so keeping *some* entries would name a
        // kind the value does not have. `object` is the kind above it.
        assert_eq!(label, "object");
    }

    #[test]
    fn a_glance_drops_union_members_into_an_any_tail() {
        let wide = Kind::either(vec![
            Kind::None,
            Kind::String,
            Kind::Int,
            Kind::Datetime,
            Kind::Uuid,
            Kind::Decimal,
            Kind::Duration,
        ]);
        assert!(render_kind(&wide).chars().count() > 48);
        let label = render(&wide, KindContext::Glance { budget: 48 }).text;
        assert!(label.chars().count() <= 48, "over budget: {label}");
        // The tail is `any`, which covers every dropped member: the label
        // says less than the truth, never something the value cannot be.
        assert!(label.ends_with(" | any>"), "got: {label}");
        assert!(!label.contains('…'), "a glance never cuts: {label}");
    }

    #[test]
    fn a_glance_never_produces_a_cut_token() {
        // Every widening step is a whole kind, so no label can end mid-name.
        let kind = Kind::Array(
            Box::new(object(&[
                ("some_quite_long_field_name", Kind::String),
                ("another_quite_long_field_name", Kind::Int),
            ])),
            Some(3),
        );
        let label = render(&kind, KindContext::Glance { budget: 20 }).text;
        assert_eq!(label, "array<object, 3>");
    }

    #[test]
    fn an_irreducible_kind_overruns_its_budget_rather_than_lying() {
        // Nothing above `datetime` is shorter, so a tiny budget gets the
        // truth instead of a prefix.
        let label = render(&Kind::Datetime, KindContext::Glance { budget: 3 }).text;
        assert_eq!(label, "datetime");
    }
}
