//! Built-in functions as completion renders them.
//!
//! The data lives on the analyzer side: [`builtin_catalog`] is the dispatch
//! table itself, one [`BuiltinEntry`] per name the engine resolves, each
//! carrying the leaf file's declared [`Signature`] and a one-line doc. This
//! module only *renders* those rows — the label, the `name(params) -> kind`
//! detail, the kinds ranking compares against — with the crate's standard
//! [`render_kind`], so a signature is spelled the same way here as in a hover
//! or a diagnostic. There is no second copy of any signature to keep in step.

use surrealdb_types::Kind;

use crate::analyzer::function::signature::{evaluate, ParamKind, ReturnKind, Signature};
use crate::analyzer::function::{builtin_catalog, BuiltinEntry};
use crate::render_kind;

/// Every built-in completion may offer: the catalog minus spellings a release
/// removed (`duration::from::days` — dispatched only so the rename hint has a
/// signature) and minus spellings SurrealDB does not document (`count::count`).
pub(crate) fn offered() -> impl Iterator<Item = &'static BuiltinEntry> {
    builtin_catalog()
        .iter()
        .filter(|entry| entry.is_current() && entry.is_documented())
}

/// `string::len(string) -> int`: the one-line signature shown as a completion
/// item's detail. An optional parameter carries a `?`; a variadic tail is
/// `...`.
pub(crate) fn signature_text(entry: &BuiltinEntry) -> String {
    let signature = entry.signature();
    let returns = return_kind_of(&signature).map_or_else(|| "any".to_string(), |k| render_kind(&k));
    format!(
        "{}({}) -> {returns}",
        entry.name,
        parameters_text(&signature)
    )
}

/// The kind a call returns when the signature states one without seeing the
/// arguments: a fixed return, a numeric aggregate, or a pass-through of a
/// parameter whose expectation names a kind. `None` when the return depends
/// on an argument the signature says nothing about (`array::first` returns
/// the element of whatever array it is given).
pub(crate) fn return_kind(entry: &BuiltinEntry) -> Option<Kind> {
    return_kind_of(&entry.signature())
}

/// The kind a method call on `receiver` returns: the signature evaluated with
/// the receiver as its first argument, so `$tags.first()` on an
/// `array<string>` ranks as a `string`. Falls back to [`return_kind`] when
/// the receiver decides nothing.
pub(crate) fn method_return_kind(entry: &BuiltinEntry, receiver: &Kind) -> Option<Kind> {
    let signature = entry.signature();
    match evaluate(&signature, std::slice::from_ref(receiver)) {
        Kind::Any => return_kind_of(&signature),
        kind => Some(kind),
    }
}

/// The kind the argument at `index` is expected to have, or `None` when the
/// signature has no expectation there (past a bounded arity, or an `any`
/// parameter).
pub(crate) fn parameter_kind(entry: &BuiltinEntry, index: usize) -> Option<Kind> {
    match entry.signature().param_at(index)? {
        ParamKind::Any => None,
        param => Some(param.kind()),
    }
}

fn return_kind_of(signature: &Signature) -> Option<Kind> {
    match &signature.return_kind {
        ReturnKind::Fixed(kind) => Some(kind.clone()),
        ReturnKind::NumericAggregate(_) => Some(Kind::Number),
        ReturnKind::SameAsArg(index) => signature.param_at(*index).map(ParamKind::kind),
        ReturnKind::ArrayElement(index) => match signature.param_at(*index)? {
            ParamKind::Exact(Kind::Array(element, _) | Kind::Set(element, _)) => {
                Some((**element).clone())
            }
            _ => None,
        },
    }
}

fn parameters_text(signature: &Signature) -> String {
    let shown = signature
        .max_args
        .unwrap_or(signature.arg_kinds.len())
        .max(signature.arg_kinds.len());
    let mut parts: Vec<String> = (0..shown)
        .map_while(|index| signature.param_at(index))
        .enumerate()
        .map(|(index, param)| {
            let mut text = render_kind(&param.kind());
            if index >= signature.min_args {
                text.push('?');
            }
            text
        })
        .collect();
    if signature.max_args.is_none() {
        parts.push("...".to_string());
    }
    parts.join(", ")
}
