use crate::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compatibility {
    Assignable,
    NotAssignable,
    Unknown,
}

impl Compatibility {
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::NotAssignable, _) | (_, Self::NotAssignable) => Self::NotAssignable,
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (Self::Assignable, Self::Assignable) => Self::Assignable,
        }
    }

    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::Assignable, _) | (_, Self::Assignable) => Self::Assignable,
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (Self::NotAssignable, Self::NotAssignable) => Self::NotAssignable,
        }
    }
}

pub fn assignability(source: &Type, target: &Type) -> Compatibility {
    use Compatibility::{Assignable, NotAssignable};
    use Type::*;

    if source == target {
        return Assignable;
    }

    match (source, target) {
        (Never, _) => Assignable,
        (_, Never) => NotAssignable,
        (Any, _) | (_, Any) => Assignable,
        (Unknown(_), _) | (_, Unknown(_)) => Compatibility::Unknown,

        (Union(source_types), _) => source_types
            .types()
            .iter()
            .map(|member| assignability(member, target))
            .fold(Assignable, Compatibility::and),
        (_, Union(target_types)) => target_types
            .types()
            .iter()
            .map(|member| assignability(source, member))
            .fold(NotAssignable, Compatibility::or),

        (None, Optional(_)) => Assignable,
        (_, Optional(inner)) => assignability(source, inner),
        (Optional(source_inner), _) => {
            assignability(source_inner, target).and(assignability(&None, target))
        }

        (Literal(literal), _) => literal_assignability(literal, target),

        (Number(source_kind), Number(target_kind)) => {
            number_assignability(*source_kind, *target_kind)
        }
        (Array(source_inner, source_len), Array(target_inner, target_len)) => {
            array_len_assignability(*source_len, *target_len)
                .and(assignability(source_inner, target_inner))
        }
        (Set(source_inner), Set(target_inner)) => assignability(source_inner, target_inner),
        (Range(source_inner), Range(target_inner)) => assignability(source_inner, target_inner),
        (Object(source_object), Object(target_object)) => {
            object_assignability(source_object, target_object)
        }
        (Record(source_tables), Record(target_tables)) => {
            if source_tables.tables().is_subset(target_tables.tables()) {
                Assignable
            } else {
                NotAssignable
            }
        }
        (Geometry(source_kind), Geometry(target_kind)) => {
            geometry_assignability(*source_kind, *target_kind)
        }

        _ => NotAssignable,
    }
}

pub fn is_assignable_to(source: &Type, target: &Type) -> bool {
    assignability(source, target) == Compatibility::Assignable
}

fn literal_assignability(literal: &LiteralType, target: &Type) -> Compatibility {
    use Compatibility::{Assignable, NotAssignable};

    match (literal, target) {
        (LiteralType::String(_), Type::String) => Assignable,
        (LiteralType::Bool(_), Type::Bool) => Assignable,
        (LiteralType::Int(_), Type::Number(NumberKind::Int | NumberKind::Number)) => Assignable,
        _ => NotAssignable,
    }
}

fn number_assignability(source: NumberKind, target: NumberKind) -> Compatibility {
    use Compatibility::{Assignable, NotAssignable};
    use NumberKind::Number;

    if source == target || target == Number {
        Assignable
    } else {
        NotAssignable
    }
}

fn array_len_assignability(source: Option<ArrayLen>, target: Option<ArrayLen>) -> Compatibility {
    use Compatibility::{Assignable, NotAssignable};

    match (source, target) {
        (_, None) => Assignable,
        (Some(source_len), Some(target_len)) if source_len == target_len => Assignable,
        _ => NotAssignable,
    }
}

fn geometry_assignability(source: GeometryKind, target: GeometryKind) -> Compatibility {
    use Compatibility::{Assignable, NotAssignable};

    if source == target || target == GeometryKind::Any {
        Assignable
    } else {
        NotAssignable
    }
}

fn object_assignability(source: &ObjectType, target: &ObjectType) -> Compatibility {
    target
        .fields()
        .iter()
        .map(|(name, target_field)| match source.field(name) {
            Some(source_field) => object_field_assignability(source_field, target_field),
            None if target_field.is_optional() => Compatibility::Assignable,
            None => Compatibility::NotAssignable,
        })
        .fold(Compatibility::Assignable, Compatibility::and)
}

fn object_field_assignability(source: &ObjectField, target: &ObjectField) -> Compatibility {
    if source.is_optional() && !target.is_optional() {
        return Compatibility::NotAssignable;
    }

    assignability(source.ty(), target.ty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int() -> Type {
        Type::Number(NumberKind::Int)
    }

    fn number() -> Type {
        Type::Number(NumberKind::Number)
    }

    fn literal_int(value: i64) -> Type {
        Type::Literal(LiteralType::Int(value))
    }

    #[test]
    fn exact_types_are_assignable() {
        assert_eq!(
            assignability(&Type::String, &Type::String),
            Compatibility::Assignable
        );
    }

    #[test]
    fn unknown_assignability_is_indeterminate_not_success() {
        assert_eq!(
            assignability(
                &Type::Unknown(UnknownReason::DynamicExpression),
                &Type::String
            ),
            Compatibility::Unknown
        );
        assert!(!is_assignable_to(
            &Type::Unknown(UnknownReason::DynamicExpression),
            &Type::String
        ));
    }

    #[test]
    fn any_is_intentionally_assignable_in_both_directions() {
        assert!(is_assignable_to(&Type::String, &Type::Any));
        assert!(is_assignable_to(&Type::Any, &Type::String));
    }

    #[test]
    fn never_is_assignable_to_every_target_but_no_value_is_assignable_to_never() {
        assert!(is_assignable_to(&Type::Never, &Type::String));
        assert!(!is_assignable_to(&Type::String, &Type::Never));
    }

    #[test]
    fn numeric_kinds_widen_to_number_but_do_not_narrow() {
        assert!(is_assignable_to(&int(), &number()));
        assert!(!is_assignable_to(&number(), &int()));
        assert!(!is_assignable_to(&Type::Number(NumberKind::Float), &int()));
    }

    #[test]
    fn literals_assign_to_their_primitive_type() {
        assert!(is_assignable_to(
            &Type::Literal(LiteralType::String("x".into())),
            &Type::String
        ));
        assert!(is_assignable_to(
            &Type::Literal(LiteralType::Bool(true)),
            &Type::Bool
        ));
        assert!(is_assignable_to(&literal_int(1), &int()));
        assert!(is_assignable_to(&literal_int(1), &number()));
    }

    #[test]
    fn optional_accepts_none_and_inner_type_but_not_null() {
        let optional_string = Type::Optional(Box::new(Type::String));

        assert!(is_assignable_to(&Type::None, &optional_string));
        assert!(is_assignable_to(&Type::String, &optional_string));
        assert!(!is_assignable_to(&Type::Null, &optional_string));
        assert!(!is_assignable_to(&optional_string, &Type::String));
    }

    #[test]
    fn union_source_must_satisfy_target_for_every_member() {
        let source = Type::Union(TypeSet::new([int(), literal_int(1)]));
        assert!(is_assignable_to(&source, &number()));

        let mixed = Type::Union(TypeSet::new([Type::String, int()]));
        assert!(!is_assignable_to(&mixed, &number()));
    }

    #[test]
    fn union_target_accepts_source_when_any_member_accepts_it() {
        let target = Type::Union(TypeSet::new([Type::String, int()]));
        assert!(is_assignable_to(&literal_int(1), &target));
        assert!(!is_assignable_to(&Type::Bool, &target));
    }

    #[test]
    fn arrays_are_element_compatible_and_exact_length_may_widen_to_unsized_array() {
        let exact_ints = Type::Array(Box::new(int()), Some(ArrayLen::Exact(2)));
        let unsized_numbers = Type::Array(Box::new(number()), None);
        let exact_numbers = Type::Array(Box::new(number()), Some(ArrayLen::Exact(2)));
        let different_len = Type::Array(Box::new(number()), Some(ArrayLen::Exact(3)));

        assert!(is_assignable_to(&exact_ints, &unsized_numbers));
        assert!(is_assignable_to(&exact_ints, &exact_numbers));
        assert!(!is_assignable_to(&unsized_numbers, &exact_numbers));
        assert!(!is_assignable_to(&exact_ints, &different_len));
    }

    #[test]
    fn records_are_assignable_when_source_tables_are_subset_of_target_tables() {
        let user_record = Type::Record(TableSet::one("user"));
        let user_or_org_record = Type::Record(TableSet::new(["user", "org"]));

        assert!(is_assignable_to(&user_record, &user_or_org_record));
        assert!(!is_assignable_to(&user_or_org_record, &user_record));
    }

    #[test]
    fn objects_are_structural_and_allow_extra_source_fields() {
        let source = Type::Object(
            ObjectType::new()
                .with_field("name", ObjectField::new(Type::String))
                .with_field("age", ObjectField::new(int())),
        );
        let target =
            Type::Object(ObjectType::new().with_field("name", ObjectField::new(Type::String)));

        assert!(is_assignable_to(&source, &target));
    }

    #[test]
    fn objects_reject_missing_required_fields_but_allow_missing_optional_fields() {
        let source =
            Type::Object(ObjectType::new().with_field("name", ObjectField::new(Type::String)));
        let target_required_age = Type::Object(
            ObjectType::new()
                .with_field("name", ObjectField::new(Type::String))
                .with_field("age", ObjectField::new(int())),
        );
        let target_optional_age = Type::Object(
            ObjectType::new()
                .with_field("name", ObjectField::new(Type::String))
                .with_field("age", ObjectField::new(int()).optional(true)),
        );

        assert!(!is_assignable_to(&source, &target_required_age));
        assert!(is_assignable_to(&source, &target_optional_age));
    }

    #[test]
    fn objects_reject_optional_source_field_for_required_target_field() {
        let source = Type::Object(
            ObjectType::new().with_field("name", ObjectField::new(Type::String).optional(true)),
        );
        let target =
            Type::Object(ObjectType::new().with_field("name", ObjectField::new(Type::String)));

        assert!(!is_assignable_to(&source, &target));
    }
}
