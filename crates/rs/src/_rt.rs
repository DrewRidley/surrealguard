//! Support the `query!` expansion resolves against.
//!
//! Hidden: not a stable API surface. Everything here exists so a generated
//! type can be written as a self-contained block that names only
//! `::surrealguard_rs::_rt::*`, and so the crate calling the macro needs no
//! dependency but `surrealguard-rs`.

pub use surrealdb_types::{
    Bytes, Datetime, Decimal, Duration, Error, Geometry, Kind, Number, Object, RecordId, Regex,
    SurrealValue, Uuid, Value,
};

/// Decoding one statement's result failed.
///
/// Carries the statement index so [`Query::decode`](crate::Query::decode) can
/// attribute the failure; the query text is attached there, since the generated
/// decode closure does not carry it.
#[derive(Debug)]
pub struct DecodeError {
    /// The zero-based index of the statement whose result failed.
    pub statement: usize,
    /// The underlying conversion failure.
    pub source: Error,
}

/// What a generated decode closure returns.
pub type DecodeResult<T> = Result<T, DecodeError>;

/// Decodes the value at `statement` into `T`, consuming it out of `values`.
///
/// The generated closure calls this once per responding statement. Slots are
/// indexed by *top-level statement*, matching the SDK: a non-responding
/// statement (a bare `LET`, a `DEFINE`) still occupies one.
///
/// # Errors
///
/// Returns [`DecodeError`] if the slot is missing or its value does not match
/// `T`.
pub fn decode_at<T: SurrealValue>(values: &mut [Value], statement: usize) -> DecodeResult<T> {
    let slot = values.get_mut(statement).ok_or_else(|| DecodeError {
        statement,
        source: Error::internal(format!(
            "the response has no result for statement {statement}"
        )),
    })?;
    T::from_value(std::mem::replace(slot, Value::None)).map_err(|source| DecodeError {
        statement,
        source,
    })
}

/// Reads field `name` out of `object` and decodes it into `T`.
///
/// Generated struct decoders call this per field. Two things it does that a
/// bare `T::from_value` would not:
///
/// - An **absent** field decodes as `Value::None`, so a `NONE`-valued column
///   the server omits still lands in an `Option<T>` as `None`.
/// - A **`NULL`** field decodes as `None` when `T` admits it. The SDK's
///   `Option<T>` accepts only `Value::None`, not `Value::Null`, so without this
///   a SurrealQL `NULL` in an `option<T>` column would be a runtime error.
///
/// The error names the field, which the SDK's bare "Expected string, got
/// record" does not.
///
/// # Errors
///
/// Returns [`Error`] if the field's value does not match `T`.
pub fn field<T: SurrealValue>(object: &mut Object, name: &str) -> Result<T, Error> {
    let mut value = object.remove(name).unwrap_or(Value::None);
    if matches!(value, Value::Null) && T::is_value(&Value::None) {
        value = Value::None;
    }
    T::from_value(value)
        .map_err(|source| Error::internal(format!("field `{name}`: {}", source.message())))
}

/// Pins a bound parameter's value to `T`, the Rust type the kind the analyzer
/// inferred for that parameter decodes into.
///
/// `Into<T>` rather than a bare `T` so the obvious literals work — `"ada"` for a
/// `string` parameter, `18` for an `int` — while an unrelated type is still a
/// type error at the call site. This is the only path by which a value reaches
/// a query, so there is no unchecked bind.
pub fn checked<T: SurrealValue, V: Into<T>>(value: V) -> T {
    value.into()
}

/// Rejects a value that is not an object, for a generated struct's decoder.
///
/// # Errors
///
/// Returns [`Error`] if `value` is not a [`Value::Object`].
pub fn expect_object(value: Value) -> Result<Object, Error> {
    match value {
        Value::Object(object) => Ok(object),
        other => Err(Error::internal(format!(
            "expected an object, got {}",
            other.kind()
        ))),
    }
}
