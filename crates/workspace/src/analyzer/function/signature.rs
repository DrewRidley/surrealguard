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
use surrealguard_syntax::ast;
use surrealguard_syntax::span::SourceSpan;

use crate::analyzer::context::AnalysisContext;

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
pub struct Signature {
    pub(crate) min_args: usize,
    /// `None` means unbounded (variadic past `arg_kinds.last()`).
    pub(crate) max_args: Option<usize>,
    /// Positional expectations. When `args.len() > arg_kinds.len()`,
    /// trailing arguments repeat `arg_kinds.last()` (the common variadic
    /// shape).
    pub(crate) arg_kinds: Vec<ParamKind>,
    pub(crate) return_kind: ReturnKind,
}

pub enum ParamKind {
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

pub enum ReturnKind {
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

/// Checks the call against `signature` (emitting findings 5002/5003 for
/// arity and argument-kind violations) and returns the inferred kind.
/// Checking never affects the returned kind — that is [`evaluate`]'s,
/// which runs regardless.
///
/// Synthetic calls (method-call sugar dispatching by name, carrying no
/// argument expressions or spans) are inference-only: nothing to anchor a
/// finding to, and the receiver-kind probe intentionally tries families.
pub fn apply(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    signature: &Signature,
    args: &[Kind],
) -> Kind {
    if !is_synthetic(call) {
        let arity_ok = arity_matches(signature, args.len());
        check_arity(ctx, call, signature, args.len());
        check_argument_kinds(ctx, call, signature, args, arity_ok);
    }
    evaluate(signature, args)
}

fn is_synthetic(call: &ast::Call) -> bool {
    call.path.span.start() == call.path.span.end()
}

fn arity_matches(signature: &Signature, found: usize) -> bool {
    found >= signature.min_args && signature.max_args.is_none_or(|max| found <= max)
}

fn check_arity(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    signature: &Signature,
    found: usize,
) {
    let expected = match (signature.min_args, signature.max_args) {
        (min, Some(max)) if found >= min && found <= max => return,
        (min, None) if found >= min => return,
        (min, Some(max)) if min == max => format!("{min} {}", plural("argument", min)),
        (min, Some(max)) => format!("{min} to {max} arguments"),
        (min, None) => format!("at least {min} {}", plural("argument", min)),
    };
    let span = SourceSpan::new(ctx.source().clone(), call.path.span);
    ctx.emit(surrealguard_diagnostics::catalog::finding(
        span,
        5002,
        format!(
            "`{}` takes {expected}, but this call passes {found}",
            call.path.node
        ),
    ));
}

fn check_argument_kinds(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    signature: &Signature,
    args: &[Kind],
    arity_ok: bool,
) {
    for (index, kind) in args.iter().enumerate() {
        // Trailing variadic expectations are resolved below; unbound
        // parameters take the expectation as a constraint, not a finding.
        if *kind == Kind::Any {
            // A mis-called function constrains nothing (arguments may be
            // shifted), and an `Any` expectation carries no information.
            if arity_ok {
                if let Some(arg_expr) = call.args.get(index) {
                    if let ast::Expr::Param(param) = &arg_expr.node {
                        if ctx.env().let_fact(param).is_none() {
                            let expected = signature.arg_kinds.get(index).or_else(|| {
                                signature
                                    .max_args
                                    .is_none()
                                    .then(|| signature.arg_kinds.last())
                                    .flatten()
                            });
                            if let Some(expected) = expected {
                                let constraint = param_kind_to_kind(expected);
                                if constraint != Kind::Any {
                                    let span = SourceSpan::new(ctx.source().clone(), arg_expr.span);
                                    ctx.constrain_param(param, span, constraint, None);
                                }
                            }
                        }
                    }
                }
            }
            continue;
        }
        // Trailing variadic arguments repeat the last expectation.
        let expected = match signature.arg_kinds.get(index) {
            Some(expected) => expected,
            None if signature.max_args.is_none() => match signature.arg_kinds.last() {
                Some(expected) => expected,
                None => continue,
            },
            // Excess arguments were already reported by arity.
            None => continue,
        };
        if param_matches(expected, kind) {
            continue;
        }
        // Anchor on the argument expression itself when the call carries
        // one; excess-argument positions without expressions are skipped.
        let Some(arg_expr) = call.args.get(index) else {
            continue;
        };
        let span = SourceSpan::new(ctx.source().clone(), arg_expr.span);
        ctx.emit(surrealguard_diagnostics::catalog::finding(
            span,
            5002,
            format!(
                "argument {} to `{}` is a `{}`, but {} is required",
                index + 1,
                call.path.node,
                crate::render_kind(kind),
                param_label(expected),
            ),
        ));
    }
}

/// Whether an argument of `kind` satisfies the expectation. Checking-side
/// twin of nothing in inference: [`evaluate`] never consults it.
fn param_matches(expected: &ParamKind, kind: &Kind) -> bool {
    let base = crate::kinds::literal_base_kind(kind).unwrap_or_else(|| kind.clone());
    match expected {
        ParamKind::Exact(target) => crate::kinds::kind_is_assignable_to(kind, target),
        ParamKind::Numeric => {
            matches!(base, Kind::Int | Kind::Float | Kind::Decimal | Kind::Number)
        }
        ParamKind::Array => matches!(base, Kind::Array(_, _) | Kind::Set(_, _)),
        ParamKind::Object => matches!(base, Kind::Object),
        ParamKind::Any => true,
    }
}

/// The `Kind` a signature expectation constrains an unbound parameter to.
fn param_kind_to_kind(expected: &ParamKind) -> Kind {
    match expected {
        ParamKind::Exact(kind) => kind.clone(),
        ParamKind::Numeric => Kind::Number,
        ParamKind::Array => Kind::Array(Box::new(Kind::Any), None),
        ParamKind::Object => Kind::Object,
        ParamKind::Any => Kind::Any,
    }
}

fn param_label(expected: &ParamKind) -> String {
    match expected {
        ParamKind::Exact(kind) => format!("`{}`", crate::render_kind(kind)),
        ParamKind::Numeric => "a number".to_string(),
        ParamKind::Array => "an array".to_string(),
        ParamKind::Object => "an object".to_string(),
        ParamKind::Any => "any value".to_string(),
    }
}

fn plural(word: &str, count: usize) -> String {
    if count == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

/// Infers the call's return kind from `signature` and the argument kinds.
///
/// Never rejects: argument mistakes are invariant violations, not type
/// ambiguity. `Kind::Any` means the return depends on an argument that is
/// missing or whose element kind can't be read.
pub fn evaluate(signature: &Signature, args: &[Kind]) -> Kind {
    match &signature.return_kind {
        ReturnKind::Fixed(kind) => kind.clone(),
        ReturnKind::SameAsArg(index) => args.get(*index).cloned().unwrap_or(Kind::Any),
        ReturnKind::ArrayElement(index) => match args.get(*index) {
            Some(Kind::Array(element, _) | Kind::Set(element, _)) => (**element).clone(),
            _ => Kind::Any,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::function::signature::evaluate;

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
