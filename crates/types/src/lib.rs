use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Type {
    Any,
    Unknown(UnknownReason),
    Never,
    None,
    Null,
    Bool,
    String,
    Number(NumberKind),
    Datetime,
    Duration,
    Uuid,
    Bytes,
    Regex,
    Range(Box<Type>),
    Array(Box<Type>, Option<ArrayLen>),
    Set(Box<Type>),
    Object(ObjectType),
    Record(TableSet),
    Geometry(GeometryKind),
    Optional(Box<Type>),
    Union(TypeSet),
    Literal(LiteralType),
    Generic(GenericType),
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Any => f.write_str("any"),
            Type::Unknown(_) => f.write_str("unknown"),
            Type::Never => f.write_str("never"),
            Type::None => f.write_str("none"),
            Type::Null => f.write_str("null"),
            Type::Bool => f.write_str("bool"),
            Type::String => f.write_str("string"),
            Type::Number(kind) => write!(f, "{kind}"),
            Type::Datetime => f.write_str("datetime"),
            Type::Duration => f.write_str("duration"),
            Type::Uuid => f.write_str("uuid"),
            Type::Bytes => f.write_str("bytes"),
            Type::Regex => f.write_str("regex"),
            Type::Range(inner) => write!(f, "range<{inner}>"),
            Type::Array(inner, None) => write!(f, "array<{inner}>"),
            Type::Array(inner, Some(len)) => write!(f, "array<{inner}; {len}>"),
            Type::Set(inner) => write!(f, "set<{inner}>"),
            Type::Object(_) => f.write_str("object"),
            Type::Record(tables) => write!(f, "record<{tables}>"),
            Type::Geometry(kind) => write!(f, "geometry<{kind}>"),
            Type::Optional(inner) => write!(f, "{inner}?"),
            Type::Union(types) => write!(f, "{types}"),
            Type::Literal(literal) => write!(f, "{literal}"),
            Type::Generic(generic) => write!(f, "${}", generic.name()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum UnknownReason {
    Unresolved,
    DynamicExpression,
    UnsupportedSyntax(String),
    LossyImport(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum NumberKind {
    Number,
    Int,
    Float,
    Decimal,
}

impl fmt::Display for NumberKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NumberKind::Number => f.write_str("number"),
            NumberKind::Int => f.write_str("int"),
            NumberKind::Float => f.write_str("float"),
            NumberKind::Decimal => f.write_str("decimal"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ArrayLen {
    Exact(u32),
}

impl fmt::Display for ArrayLen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArrayLen::Exact(len) => write!(f, "{len}"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObjectType {
    fields: BTreeMap<String, ObjectField>,
}

impl ObjectType {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_field(mut self, name: impl Into<String>, field: ObjectField) -> Self {
        self.fields.insert(name.into(), field);
        self
    }

    pub fn field(&self, name: &str) -> Option<&ObjectField> {
        self.fields.get(name)
    }

    pub fn fields(&self) -> &BTreeMap<String, ObjectField> {
        &self.fields
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObjectField {
    ty: Type,
    optional: bool,
    readonly: bool,
    permissions: FieldPermissions,
}

impl ObjectField {
    pub fn new(ty: Type) -> Self {
        Self {
            ty,
            optional: false,
            readonly: false,
            permissions: FieldPermissions::default(),
        }
    }

    pub fn optional(mut self, optional: bool) -> Self {
        self.optional = optional;
        self
    }

    pub fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    pub fn with_permissions(mut self, permissions: FieldPermissions) -> Self {
        self.permissions = permissions;
        self
    }

    pub fn ty(&self) -> &Type {
        &self.ty
    }

    pub fn is_optional(&self) -> bool {
        self.optional
    }

    pub fn is_readonly(&self) -> bool {
        self.readonly
    }

    pub fn permissions(&self) -> &FieldPermissions {
        &self.permissions
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FieldPermissions {
    readable: bool,
    writable: bool,
}

impl FieldPermissions {
    pub fn new(readable: bool, writable: bool) -> Self {
        Self { readable, writable }
    }

    pub fn read_only() -> Self {
        Self::new(true, false)
    }

    pub fn read_write() -> Self {
        Self::new(true, true)
    }

    pub fn none() -> Self {
        Self::new(false, false)
    }

    pub fn readable(self) -> bool {
        self.readable
    }

    pub fn writable(self) -> bool {
        self.writable
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TableSet {
    tables: BTreeSet<String>,
}

impl TableSet {
    pub fn one(table: impl Into<String>) -> Self {
        let mut tables = BTreeSet::new();
        tables.insert(table.into());
        Self { tables }
    }

    pub fn tables(&self) -> &BTreeSet<String> {
        &self.tables
    }
}

impl fmt::Display for TableSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let joined = self.tables.iter().cloned().collect::<Vec<_>>().join(" | ");
        f.write_str(&joined)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum GeometryKind {
    Any,
    Point,
    Line,
    Polygon,
}

impl fmt::Display for GeometryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GeometryKind::Any => f.write_str("any"),
            GeometryKind::Point => f.write_str("point"),
            GeometryKind::Line => f.write_str("line"),
            GeometryKind::Polygon => f.write_str("polygon"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TypeSet {
    types: BTreeSet<Type>,
}

impl TypeSet {
    pub fn new(types: impl IntoIterator<Item = Type>) -> Self {
        Self {
            types: types.into_iter().collect(),
        }
    }

    pub fn types(&self) -> &BTreeSet<Type> {
        &self.types
    }
}

impl fmt::Display for TypeSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let joined = self
            .types
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" | ");
        f.write_str(&joined)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum LiteralType {
    String(String),
    Bool(bool),
    Int(i64),
}

impl fmt::Display for LiteralType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LiteralType::String(value) => write!(f, "{value:?}"),
            LiteralType::Bool(value) => write!(f, "{value}"),
            LiteralType::Int(value) => write!(f, "{value}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GenericType {
    name: String,
}

impl GenericType {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_and_unknown_are_distinct() {
        assert_ne!(Type::Any, Type::Unknown(UnknownReason::Unresolved));
    }

    #[test]
    fn optional_type_formats_predictably() {
        assert_eq!(Type::Optional(Box::new(Type::String)).to_string(), "string?");
    }

    #[test]
    fn object_fields_preserve_static_metadata() {
        let field = ObjectField::new(Type::String)
            .optional(true)
            .readonly(true)
            .with_permissions(FieldPermissions::read_only());
        let object = ObjectType::new().with_field("name", field.clone());

        assert_eq!(object.field("name"), Some(&field));
        assert!(object.field("name").expect("field exists").is_optional());
        assert!(object.field("name").expect("field exists").is_readonly());
        assert_eq!(
            object
                .field("name")
                .expect("field exists")
                .permissions(),
            &FieldPermissions::read_only()
        );
    }
}
