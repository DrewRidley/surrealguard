/// Type system — re-exports surrealdb's `Kind` as our canonical type
/// representation, with extension helpers for analysis.
///
/// Using surrealdb's types directly means:
/// - Direct compatibility with surrealdb schema definitions
/// - No maintenance burden keeping types in sync
/// - Future codegen works with `Kind` directly
///
/// ## Regex representation
///
/// surrealdb's `Kind` enum does not include a `Regex` variant. We repurpose
/// `Kind::Point` (which the analyzer never uses for its original meaning —
/// points are represented as `Kind::Geometry(vec!["point"])`) to represent
/// the regex type internally. Use [`regex_kind()`] and [`is_regex()`] helpers
/// instead of matching `Kind::Point` directly.
use std::collections::BTreeMap;
use std::fmt;

pub use surrealdb::sql::Kind;
pub use surrealdb::sql::Literal;
pub use surrealdb::sql::Table;

/// Returns a `Kind` representing the regex type.
///
/// Internally uses `Kind::Point` as a sentinel since surrealdb's `Kind`
/// does not have a `Regex` variant.
pub fn regex_kind() -> Kind {
    Kind::Point
}

/// Returns true if the given kind represents a regex type.
pub fn is_regex(kind: &Kind) -> bool {
    matches!(kind, Kind::Point)
}

/// Display a `Kind` with analyzer-specific type names.
///
/// This handles the `Kind::Point` → "regex" mapping so that diagnostics
/// show the correct type name.
pub fn display_kind(kind: &Kind) -> String {
    if is_regex(kind) {
        "regex".to_string()
    } else {
        format!("{}", kind)
    }
}

/// Wrapper for displaying a `Kind` with regex support.
pub struct DisplayKind<'a>(pub &'a Kind);

impl<'a> fmt::Display for DisplayKind<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&display_kind(self.0))
    }
}

/// Extension trait adding analysis helpers to `Kind`.
///
/// surrealdb's helper methods are `pub(crate)`, so we provide
/// our own for use in the analyzer.
pub trait KindExt {
    /// Returns true if this type is `Any`.
    fn is_any(&self) -> bool;

    /// Returns true if this type is nullable (Option or Null).
    fn is_nullable(&self) -> bool;

    /// Wraps this type in Option if not already nullable.
    fn optional(self) -> Kind;

    /// Returns true if this is a numeric type.
    fn is_numeric(&self) -> bool;

    /// Returns true if this is a record type.
    fn is_record(&self) -> bool;

    /// Build a typed object as `Literal::Object`.
    fn typed_object(fields: BTreeMap<String, Kind>) -> Kind;
}

impl KindExt for Kind {
    fn is_any(&self) -> bool {
        matches!(self, Kind::Any)
    }

    fn is_nullable(&self) -> bool {
        matches!(self, Kind::Null | Kind::Option(_))
    }

    fn optional(self) -> Kind {
        match self {
            Kind::Null | Kind::Option(_) => self,
            other => Kind::Option(Box::new(other)),
        }
    }

    fn is_numeric(&self) -> bool {
        matches!(
            self,
            Kind::Int | Kind::Float | Kind::Decimal | Kind::Number
        )
    }

    fn is_record(&self) -> bool {
        matches!(self, Kind::Record(_))
    }

    fn typed_object(fields: BTreeMap<String, Kind>) -> Kind {
        Kind::Literal(Literal::Object(fields))
    }
}

/// Returns true if the type is definitely a scalar (not an object or record).
///
/// This is used to validate CONTENT clauses, which require an object value.
/// Returns false for `Any`, `Object`, `Record`, `Literal(Object(_))`, `Option`,
/// `Either`, and other types that *could* be objects.
pub fn is_definitely_not_object(kind: &Kind) -> bool {
    matches!(
        kind,
        Kind::String
            | Kind::Int
            | Kind::Float
            | Kind::Decimal
            | Kind::Number
            | Kind::Bool
            | Kind::Null
            | Kind::Datetime
            | Kind::Duration
            | Kind::Uuid
            | Kind::Bytes
            | Kind::Geometry(_)
            | Kind::Array(_, _)
            | Kind::Set(_, _)
            | Kind::Range
            | Kind::Point
    )
}

/// Check if a value of type `value` can be assigned to a target of type `target`.
///
/// Returns true if the assignment is valid. This is not strict equality —
/// it allows coercion (e.g., int → number) and accepts Any on either side.
pub fn is_assignable(target: &Kind, value: &Kind) -> bool {
    // Any accepts anything, anything can satisfy Any
    if matches!(target, Kind::Any) || matches!(value, Kind::Any) {
        return true;
    }

    // Exact match
    if target == value {
        return true;
    }

    match (target, value) {
        // Range is only assignable to Range (handled by exact match above)
        (Kind::Range, _) | (_, Kind::Range) => false,

        // Regex (Kind::Point sentinel) is only assignable to Regex
        // (handled by exact match above)
        (t, _) if is_regex(t) => false,
        (_, v) if is_regex(v) => false,

        // Null can be assigned to Option<T>
        (Kind::Option(_), Kind::Null) => true,
        // T can be assigned to Option<T>
        (Kind::Option(inner), val) => is_assignable(inner, val),
        // Option<T> can be assigned to T (will just be non-null at runtime)
        (target, Kind::Option(inner)) => is_assignable(target, inner),

        // Numeric coercion: all numeric types are interassignable
        // SurrealDB coerces at runtime, so int↔number↔float↔decimal all work
        (Kind::Number, Kind::Int | Kind::Float | Kind::Decimal) => true,
        (Kind::Int, Kind::Number | Kind::Float | Kind::Decimal) => true,
        (Kind::Float, Kind::Int | Kind::Number | Kind::Decimal) => true,
        (Kind::Decimal, Kind::Int | Kind::Float | Kind::Number) => true,

        // Array element compatibility
        (Kind::Array(target_inner, _), Kind::Array(value_inner, _)) => {
            is_assignable(target_inner, value_inner)
        }

        // Set element compatibility
        (Kind::Set(target_inner, _), Kind::Set(value_inner, _)) => {
            is_assignable(target_inner, value_inner)
        }

        // Record type compatibility: record<specific> accepts record<specific>
        // record<> (any record) accepts any record
        (Kind::Record(target_tables), Kind::Record(value_tables)) => {
            if target_tables.is_empty() {
                return true; // record<> accepts any record
            }
            if value_tables.is_empty() {
                return true; // untyped record, can't validate
            }
            // Every value table must be in target tables
            value_tables
                .iter()
                .all(|vt| target_tables.iter().any(|tt| tt == vt))
        }

        // Object literal assignability: check each field
        (Kind::Literal(Literal::Object(target_fields)), Kind::Literal(Literal::Object(value_fields))) => {
            // All target fields must be satisfiable by value fields
            target_fields.iter().all(|(name, target_type)| {
                match value_fields.get(name) {
                    Some(value_type) => is_assignable(target_type, value_type),
                    None => target_type.is_nullable(), // missing field ok if optional
                }
            })
        }

        // Object accepts any object literal
        (Kind::Object, Kind::Literal(Literal::Object(_))) => true,
        (Kind::Literal(Literal::Object(_)), Kind::Object) => true,

        // Geometry subtype covariance:
        // - geometry (bare) accepts any geometry subtype
        // - geometry<collection> accepts any geometry subtype
        // - geometry<point> is NOT assignable to geometry<line> (different subtypes)
        (Kind::Geometry(target_variants), Kind::Geometry(value_variants)) => {
            // bare geometry (empty vec) accepts any geometry
            if target_variants.is_empty() {
                return true;
            }
            // geometry<collection> accepts any geometry subtype
            if target_variants.iter().any(|v| v == "collection") {
                return true;
            }
            // bare geometry value can satisfy any geometry target (unknown specifics)
            if value_variants.is_empty() {
                return true;
            }
            // Every value variant must appear in target variants
            value_variants
                .iter()
                .all(|vv| target_variants.iter().any(|tv| tv == vv))
        }

        // Either/union: value matches if it matches any variant
        (Kind::Either(variants), val) => variants.iter().any(|v| is_assignable(v, val)),
        (target, Kind::Either(variants)) => variants.iter().all(|v| is_assignable(target, v)),

        _ => false,
    }
}

/// Check if a cast from `source` to `target` is valid.
///
/// Returns true for legal casts. Some casts are always valid (int → string),
/// some are conditional (string → int may fail at runtime), and some are
/// nonsensical (geometry → bool).
pub fn is_valid_cast(target: &Kind, source: &Kind) -> bool {
    if matches!(target, Kind::Any) || matches!(source, Kind::Any) {
        return true;
    }
    if target == source {
        return true;
    }

    // Range cannot be cast to/from other types (only range→range, handled above)
    if matches!(target, Kind::Range) || matches!(source, Kind::Range) {
        return false;
    }

    // Regex cast rules: string→regex and regex→string
    if is_regex(target) {
        return matches!(source, Kind::String) || is_regex(source);
    }
    if is_regex(source) {
        return matches!(target, Kind::String);
    }

    match target {
        // Everything can be cast to string (serialization)
        Kind::String => true,
        // Everything can be cast to bool (truthiness)
        Kind::Bool => true,
        // Numeric casts: from string, numeric types
        Kind::Int | Kind::Float | Kind::Decimal | Kind::Number => matches!(
            source,
            Kind::String
                | Kind::Int
                | Kind::Float
                | Kind::Decimal
                | Kind::Number
                | Kind::Bool
                | Kind::Duration
        ),
        // Duration from string or numeric
        Kind::Duration => matches!(
            source,
            Kind::String | Kind::Int | Kind::Float | Kind::Number | Kind::Duration
        ),
        // Datetime from string
        Kind::Datetime => matches!(source, Kind::String | Kind::Datetime | Kind::Int),
        // UUID from string
        Kind::Uuid => matches!(source, Kind::String | Kind::Uuid),
        // Record from string or record
        Kind::Record(_) => matches!(source, Kind::String | Kind::Record(_)),
        // Array/Set casts
        Kind::Array(_, _) => matches!(source, Kind::Array(_, _) | Kind::Set(_, _)),
        Kind::Set(_, _) => matches!(source, Kind::Array(_, _) | Kind::Set(_, _)),
        // Object from string
        Kind::Object => matches!(source, Kind::String | Kind::Object | Kind::Literal(Literal::Object(_))),
        // Bytes from string
        Kind::Bytes => matches!(source, Kind::String | Kind::Bytes),
        // Geometry conversions: geometry subtypes are covariant (same rules as assignability)
        Kind::Geometry(_) => match source {
            Kind::Geometry(_) => is_assignable(target, source),
            Kind::Array(_, _) => true,
            _ => false,
        },
        // Null cast
        Kind::Null => true,
        // Option
        Kind::Option(inner) => is_valid_cast(inner, source),
        // Anything else
        _ => false,
    }
}

/// Check if a value of type `value` is valid for a compound assignment operator
/// (`+=` or `-=`) on a field of type `target`.
///
/// For `+=`:
/// - If target is array<T>, value should be T (append) or array<T> (concat)
/// - If target is numeric, value should be numeric (addition)
/// - If target is string, value should be string (concatenation)
///
/// For `-=`:
/// - If target is array<T>, value should be T (remove) or array<T> (remove all)
/// - If target is numeric, value should be numeric (subtraction)
pub fn is_compound_assignable(target: &Kind, value: &Kind, operator: &str) -> bool {
    // Any on either side always passes
    if matches!(target, Kind::Any) || matches!(value, Kind::Any) {
        return true;
    }

    match operator {
        "+=" => {
            match target {
                // array<T> += T (append) or array<T> += array<T> (concat)
                Kind::Array(inner, _) => {
                    is_assignable(inner, value) || is_assignable(target, value)
                }
                // set<T> += T (add) or set<T> += set<T> (union)
                Kind::Set(inner, _) => {
                    is_assignable(inner, value) || is_assignable(target, value)
                }
                // numeric += numeric
                t if t.is_numeric() => value.is_numeric(),
                // string += string
                Kind::String => matches!(value, Kind::String),
                // Option<T> — unwrap and check
                Kind::Option(inner) => is_compound_assignable(inner, value, operator),
                _ => false,
            }
        }
        "-=" => {
            match target {
                // array<T> -= T (remove) or array<T> -= array<T> (remove all)
                Kind::Array(inner, _) => {
                    is_assignable(inner, value) || is_assignable(target, value)
                }
                Kind::Set(inner, _) => {
                    is_assignable(inner, value) || is_assignable(target, value)
                }
                // numeric -= numeric
                t if t.is_numeric() => value.is_numeric(),
                // Option<T> — unwrap and check
                Kind::Option(inner) => is_compound_assignable(inner, value, operator),
                _ => false,
            }
        }
        // +?= is "assign if not set" — same rules as =
        "+?=" => is_assignable(target, value),
        // = is standard assignment
        "=" => is_assignable(target, value),
        _ => is_assignable(target, value),
    }
}

/// Helper to create a `Table` from a string.
pub fn table(name: &str) -> Table {
    Table::from(name.to_string())
}

/// Helper to create a record Kind from table name strings.
pub fn record_kind(tables: &[&str]) -> Kind {
    Kind::Record(tables.iter().map(|t| table(t)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Range assignability tests ──────────────────────────────

    #[test]
    fn range_assignable_to_range() {
        assert!(is_assignable(&Kind::Range, &Kind::Range));
    }

    #[test]
    fn range_assignable_to_any() {
        assert!(is_assignable(&Kind::Any, &Kind::Range));
    }

    #[test]
    fn any_assignable_to_range() {
        assert!(is_assignable(&Kind::Range, &Kind::Any));
    }

    #[test]
    fn range_not_assignable_to_string() {
        assert!(!is_assignable(&Kind::String, &Kind::Range));
    }

    #[test]
    fn string_not_assignable_to_range() {
        assert!(!is_assignable(&Kind::Range, &Kind::String));
    }

    #[test]
    fn range_not_assignable_to_int() {
        assert!(!is_assignable(&Kind::Int, &Kind::Range));
    }

    #[test]
    fn range_not_assignable_to_bool() {
        assert!(!is_assignable(&Kind::Bool, &Kind::Range));
    }

    // ── Range cast tests ──────────────────────────────────────

    #[test]
    fn range_cast_to_range() {
        assert!(is_valid_cast(&Kind::Range, &Kind::Range));
    }

    #[test]
    fn range_cannot_cast_to_string() {
        assert!(!is_valid_cast(&Kind::String, &Kind::Range));
    }

    #[test]
    fn string_cannot_cast_to_range() {
        assert!(!is_valid_cast(&Kind::Range, &Kind::String));
    }

    #[test]
    fn range_cannot_cast_to_int() {
        assert!(!is_valid_cast(&Kind::Int, &Kind::Range));
    }

    // ── Regex assignability tests ─────────────────────────────

    #[test]
    fn regex_assignable_to_regex() {
        assert!(is_assignable(&regex_kind(), &regex_kind()));
    }

    #[test]
    fn regex_assignable_to_any() {
        assert!(is_assignable(&Kind::Any, &regex_kind()));
    }

    #[test]
    fn any_assignable_to_regex() {
        assert!(is_assignable(&regex_kind(), &Kind::Any));
    }

    #[test]
    fn regex_not_assignable_to_string() {
        assert!(!is_assignable(&Kind::String, &regex_kind()));
    }

    #[test]
    fn string_not_assignable_to_regex() {
        assert!(!is_assignable(&regex_kind(), &Kind::String));
    }

    #[test]
    fn regex_not_assignable_to_int() {
        assert!(!is_assignable(&Kind::Int, &regex_kind()));
    }

    // ── Regex cast tests ──────────────────────────────────────

    #[test]
    fn string_can_cast_to_regex() {
        assert!(is_valid_cast(&regex_kind(), &Kind::String));
    }

    #[test]
    fn regex_can_cast_to_string() {
        assert!(is_valid_cast(&Kind::String, &regex_kind()));
    }

    #[test]
    fn regex_cannot_cast_to_int() {
        assert!(!is_valid_cast(&Kind::Int, &regex_kind()));
    }

    #[test]
    fn int_cannot_cast_to_regex() {
        assert!(!is_valid_cast(&regex_kind(), &Kind::Int));
    }

    #[test]
    fn regex_cast_to_regex() {
        assert!(is_valid_cast(&regex_kind(), &regex_kind()));
    }

    // ── Display tests ─────────────────────────────────────────

    #[test]
    fn display_regex_kind() {
        assert_eq!(display_kind(&regex_kind()), "regex");
    }

    #[test]
    fn display_string_kind() {
        assert_eq!(display_kind(&Kind::String), "string");
    }

    #[test]
    fn display_range_kind() {
        assert_eq!(display_kind(&Kind::Range), "range");
    }
}
