//! Declarative signatures for built-in function analyzers.
//!
//! A `Signature` records what a function expects (arity, per-argument
//! kinds) and how its return kind derives from the arguments. Inference
//! and checking are separate concerns with separate consumers:
//!
//! - [`evaluate`] performs *inference only*: it computes the return kind
//!   and never rejects a call. A mistaken argument violates an invariant —
//!   the function still has its return type. `Kind::Any` appears only when
//!   the return genuinely depends on information that isn't there (a
//!   missing or non-collection argument that `SameAsArg`/`ArrayElement`
//!   read from).
//! - The arity and argument-kind expectations are the declarative invariant
//!   spec for diagnostics, which report violations without changing the
//!   inferred type.
//!
//! Functions with genuinely bespoke behavior (value-dependent shape,
//! unusual variadic rules) skip the table and write their own body —
//! that's why each function has its own file rather than being purely
//! table-driven.

use surrealdb_types::Kind;

/// A function's arity and per-argument kind expectations, plus how its
/// return kind is derived from the arguments.
///
/// `arg_kinds` is an owned `Vec` rather than a `&'static [ParamKind]`
/// slice: a `ParamKind::Exact(Kind::Geometry(Vec::new()))` (or any `Kind`
/// variant holding a `Vec`) can't be const-promoted to `'static` since
/// `Vec` has a `Drop` impl, even though an empty `Vec::new()` never
/// allocates — that made `&[...]` literals fail to compile for those
/// variants while working by accident for simple ones like `Kind::String`.
/// `vec![...]` sidesteps the whole class of lifetime issues.
pub(crate) struct Signature {
    // Arity and argument expectations are declarative invariant data:
    // inference never checks them; the mismatch findings that will are not
    // built yet.
    #[allow(dead_code)]
    pub(crate) min_args: usize,
    /// `None` means unbounded (variadic past `arg_kinds.last()`).
    #[allow(dead_code)]
    pub(crate) max_args: Option<usize>,
    /// Positional expectations. When `args.len() > arg_kinds.len()`,
    /// trailing arguments repeat `arg_kinds.last()` (the common variadic
    /// shape).
    #[allow(dead_code)]
    pub(crate) arg_kinds: Vec<ParamKind>,
    pub(crate) return_kind: ReturnKind,
}

// The argument expectations are declarative invariant data: nothing reads
// them during inference (which never checks), and the mismatch findings
// that will read them are not built yet.
#[allow(dead_code)]
pub(crate) enum ParamKind {
    /// Must be assignable to this exact kind (numeric widening allowed,
    /// same rule mutation field assignability already uses).
    Exact(Kind),
    /// Any of `Int`/`Float`/`Decimal`/`Number`.
    Numeric,
    /// Any `Array` or `Set`.
    Array,
    /// Any `Object`.
    Object,
    /// No constraint; still counts toward arity.
    Any,
}

pub(crate) enum ReturnKind {
    /// Always this kind, regardless of arguments.
    Fixed(Kind),
    /// Same kind as `args[index]` (e.g. `array::first` returns the array's
    /// element kind is a *different* case — see `ArrayElement` — this is
    /// for functions that pass a value through unchanged, like `math::max`
    /// picking between two same-kind numbers).
    SameAsArg(usize),
    /// The element kind of `args[index]`, when that argument is an `Array`
    /// or `Set` (e.g. `array::first`, `array::pop`).
    ArrayElement(usize),
}

/// Infers the call's return kind from `signature` and the argument kinds.
///
/// Never rejects: argument mistakes are invariant violations, not type
/// ambiguity. `Kind::Any` means the return depends on an argument that is
/// missing or whose element kind can't be read.
pub(crate) fn evaluate(signature: &Signature, args: &[Kind]) -> Kind {
    match &signature.return_kind {
        ReturnKind::Fixed(kind) => kind.clone(),
        ReturnKind::SameAsArg(index) => args.get(*index).cloned().unwrap_or(Kind::Any),
        ReturnKind::ArrayElement(index) => match args.get(*index) {
            Some(Kind::Array(element, _)) | Some(Kind::Set(element, _)) => (**element).clone(),
            _ => Kind::Any,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_int() -> Signature {
        Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Exact(Kind::String)],
            return_kind: ReturnKind::Fixed(Kind::Int),
        }
    }

    #[test]
    fn fixed_returns_regardless_of_arguments() {
        // Wrong arity or kinds violate invariants; the return type is still
        // the function's return type.
        assert_eq!(evaluate(&fixed_int(), &[Kind::String]), Kind::Int);
        assert_eq!(evaluate(&fixed_int(), &[]), Kind::Int);
        assert_eq!(evaluate(&fixed_int(), &[Kind::Int, Kind::Bool]), Kind::Int);
    }

    #[test]
    fn same_as_arg_passes_the_argument_kind_through() {
        let signature = Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Numeric, ParamKind::Numeric],
            return_kind: ReturnKind::SameAsArg(0),
        };

        assert_eq!(evaluate(&signature, &[Kind::Float, Kind::Int]), Kind::Float);
        // The return depends on an argument that isn't there.
        assert_eq!(evaluate(&signature, &[]), Kind::Any);
    }

    #[test]
    fn array_element_reads_the_collection_element_kind() {
        let signature = Signature {
            min_args: 1,
            max_args: Some(1),
            arg_kinds: vec![ParamKind::Array],
            return_kind: ReturnKind::ArrayElement(0),
        };

        assert_eq!(
            evaluate(&signature, &[Kind::Array(Box::new(Kind::String), Some(3))]),
            Kind::String
        );
        // A non-collection argument gives nothing to read an element from.
        assert_eq!(evaluate(&signature, &[Kind::Int]), Kind::Any);
        assert_eq!(evaluate(&signature, &[]), Kind::Any);
    }
}
