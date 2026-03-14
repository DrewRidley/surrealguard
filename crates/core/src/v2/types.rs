/// SurrealGuard's own type system, independent of `surrealdb::sql::Kind`.
///
/// This replaces the v1 dependency on surrealdb's internal types,
/// giving us full control and stability across SurrealDB versions.
use std::collections::BTreeMap;
use std::fmt;

/// The core type representation for SurrealQL values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    /// Any type — used when we can't determine the type.
    Any,
    /// The null/none type.
    Null,
    /// Boolean.
    Bool,
    /// Integer (SurrealDB's `int`).
    Int,
    /// Floating point (SurrealDB's `float`).
    Float,
    /// Decimal (arbitrary precision).
    Decimal,
    /// Generic number — could be int, float, or decimal.
    Number,
    /// String.
    String,
    /// Binary data.
    Bytes,
    /// Duration value.
    Duration,
    /// Datetime value.
    Datetime,
    /// UUID.
    Uuid,
    /// Record ID pointing to specific table(s).
    Record(Vec<String>),
    /// Array with element type and optional max length.
    Array(Box<Type>, Option<usize>),
    /// Set with element type and optional max length.
    Set(Box<Type>, Option<usize>),
    /// Object with known field types.
    Object(BTreeMap<String, Type>),
    /// Geometry with variant names (point, line, polygon, etc.).
    Geometry(Vec<String>),
    /// A range type.
    Range,
    /// A union of possible types (e.g., `string | int`).
    Either(Vec<Type>),
    /// An option type (syntactic sugar for `T | null`).
    Option(Box<Type>),
    /// A literal/specific value type — used for exact return types.
    Literal(Literal),
    /// Function type with optional input and output types.
    Function(Option<Box<Type>>, Option<Box<Type>>),
}

/// Literal types for exact value representations in the type system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    /// A literal object with exact field types.
    Object(BTreeMap<String, Type>),
    /// A literal array of specific types (for multi-statement results).
    Array(Vec<Type>),
    /// A literal string value.
    String(String),
    /// A literal integer value.
    Int(i64),
    /// A literal boolean value.
    Bool(bool),
}

impl Type {
    /// Returns true if this type is `Any`.
    pub fn is_any(&self) -> bool {
        matches!(self, Type::Any)
    }

    /// Returns true if this type is nullable.
    pub fn is_nullable(&self) -> bool {
        matches!(self, Type::Null | Type::Option(_))
    }

    /// Wraps this type in Option if not already nullable.
    pub fn optional(self) -> Type {
        match self {
            Type::Null | Type::Option(_) => self,
            other => Type::Option(Box::new(other)),
        }
    }

    /// Returns true if this is a numeric type.
    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            Type::Int | Type::Float | Type::Decimal | Type::Number
        )
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Any => write!(f, "any"),
            Type::Null => write!(f, "null"),
            Type::Bool => write!(f, "bool"),
            Type::Int => write!(f, "int"),
            Type::Float => write!(f, "float"),
            Type::Decimal => write!(f, "decimal"),
            Type::Number => write!(f, "number"),
            Type::String => write!(f, "string"),
            Type::Bytes => write!(f, "bytes"),
            Type::Duration => write!(f, "duration"),
            Type::Datetime => write!(f, "datetime"),
            Type::Uuid => write!(f, "uuid"),
            Type::Record(tables) => {
                write!(f, "record<{}>", tables.join(" | "))
            }
            Type::Array(inner, len) => {
                if let Some(n) = len {
                    write!(f, "array<{}, {}>", inner, n)
                } else {
                    write!(f, "array<{}>", inner)
                }
            }
            Type::Set(inner, len) => {
                if let Some(n) = len {
                    write!(f, "set<{}, {}>", inner, n)
                } else {
                    write!(f, "set<{}>", inner)
                }
            }
            Type::Object(fields) => {
                let entries: Vec<_> = fields
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, v))
                    .collect();
                write!(f, "{{ {} }}", entries.join(", "))
            }
            Type::Geometry(variants) => {
                write!(f, "geometry<{}>", variants.join(" | "))
            }
            Type::Range => write!(f, "range"),
            Type::Either(types) => {
                let parts: Vec<_> = types.iter().map(|t| t.to_string()).collect();
                write!(f, "{}", parts.join(" | "))
            }
            Type::Option(inner) => write!(f, "option<{}>", inner),
            Type::Literal(lit) => write!(f, "{}", lit),
            Type::Function(input, output) => {
                let i = input
                    .as_ref()
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "any".to_string());
                let o = output
                    .as_ref()
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "any".to_string());
                write!(f, "function({}) -> {}", i, o)
            }
        }
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Object(fields) => {
                let entries: Vec<_> = fields
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, v))
                    .collect();
                write!(f, "{{ {} }}", entries.join(", "))
            }
            Literal::Array(types) => {
                let parts: Vec<_> = types.iter().map(|t| t.to_string()).collect();
                write!(f, "[{}]", parts.join(", "))
            }
            Literal::String(s) => write!(f, "\"{}\"", s),
            Literal::Int(n) => write!(f, "{}", n),
            Literal::Bool(b) => write!(f, "{}", b),
        }
    }
}
