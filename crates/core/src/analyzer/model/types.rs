use std::collections::HashMap;
use surrealdb::sql::{Expression, Ident, Part, Permissions, Table, Value};

/// Complete type definition including metadata
#[derive(Clone, Debug, PartialEq)]
pub struct Type {
    /// The fundamental type classification
    pub kind: TypeKind,
    /// Associated metadata and constraints
    pub meta: TypeMetadata,
}

impl Type {
    pub fn new(kind: TypeKind) -> Self {
        Self {
            kind,
            meta: TypeMetadata::default()
        }
    }

    pub fn with_permissions(mut self, perms: Permissions) -> Self {
        self.meta.permissions = perms;
        self
    }

    pub fn with_default(mut self, default: Value) -> Self {
        self.meta.default = Some(default);
        self
    }

    pub fn with_assert(mut self, assert: Value) -> Self {
        self.meta.assert = Some(assert);
        self
    }

    // Helper constructors
    pub fn null() -> Self { Self::new(TypeKind::Null) }
    pub fn bool() -> Self { Self::new(TypeKind::Bool) }
    pub fn int() -> Self { Self::new(TypeKind::Number) }
    pub fn float() -> Self { Self::new(TypeKind::Number) }
    pub fn decimal() -> Self { Self::new(TypeKind::Number) }
    pub fn string() -> Self { Self::new(TypeKind::String) }
    pub fn duration() -> Self { Self::new(TypeKind::Duration) }
    pub fn datetime() -> Self { Self::new(TypeKind::Datetime) }
    pub fn uuid() -> Self { Self::new(TypeKind::Uuid) }
    pub fn unknown() -> Self { Self::new(TypeKind::Unknown) }
    pub fn any() -> Self { Self::new(TypeKind::Any) }
    pub fn any_array() -> Self { Self::array(Self::any()) }

    pub fn array(inner: Type) -> Self {
        Self::new(TypeKind::Array(Box::new(inner)))
    }

    pub fn empty_object() -> Self {
        Self::object(HashMap::new())
    }

    pub fn object_with(fields: Vec<(&str, Type)>) -> Self {
            Self::object(fields.into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect())
    }

    pub fn object(fields: HashMap<String, Type>) -> Self {
        Self::new(TypeKind::Object(fields))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeMetadata {
    pub permissions: Permissions,
    pub default: Option<Value>,
    pub assert: Option<Value>,
}

impl Default for TypeMetadata {
    fn default() -> Self {
        Self {
            permissions: Permissions::none(),
            default: None,
            assert: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeKind {
    // Basic Types
    Null,
    Bool,
    Number,
    String,
    Duration,
    Datetime,
    Uuid,
    Geometry,
    Bytes,

    // Composite Types
    Array(Box<Type>),
    Object(HashMap<String, Type>),
    Union(Vec<Type>),
    Record { table: String },

    // Parameters
    Param(String),

    // Special Types
    Any,
    Unknown,
}
