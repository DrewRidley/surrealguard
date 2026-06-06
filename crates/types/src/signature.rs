use std::collections::BTreeMap;

use crate::{assignability, Compatibility, Type};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionSig {
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub params: Vec<ParamType>,
    pub returns: TypeExpr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenericParam {
    pub name: String,
}

impl GenericParam {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamType {
    Type(TypeExpr),
    Variadic(Box<ParamType>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeExpr {
    Exact(Type),
    Generic(String),
    Array(Box<TypeExpr>),
    Optional(Box<TypeExpr>),
    OneOf(Vec<TypeExpr>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureMatch {
    pub result_type: Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureError {
    ArityMismatch {
        expected: usize,
        found: usize,
    },
    TypeMismatch {
        param_index: usize,
        expected: TypeExpr,
        found: Type,
    },
    ConflictingGenericBinding {
        generic: String,
        existing: Type,
        found: Type,
    },
    UnboundGeneric {
        generic: String,
    },
}

type Bindings = BTreeMap<String, Type>;

pub fn match_signature(
    signature: &FunctionSig,
    args: &[Type],
) -> Result<SignatureMatch, SignatureError> {
    let mut bindings = Bindings::new();
    match_params(&signature.params, args, &mut bindings)?;
    let result_type = resolve_expr(&signature.returns, &bindings)?;

    Ok(SignatureMatch { result_type })
}

fn match_params(
    params: &[ParamType],
    args: &[Type],
    bindings: &mut Bindings,
) -> Result<(), SignatureError> {
    match params.last() {
        Some(ParamType::Variadic(variadic)) => {
            let fixed_len = params.len() - 1;
            if args.len() < fixed_len {
                return Err(SignatureError::ArityMismatch {
                    expected: fixed_len,
                    found: args.len(),
                });
            }

            for (index, (param, arg)) in params[..fixed_len].iter().zip(args.iter()).enumerate() {
                match_param(param, arg, index, bindings)?;
            }

            for (offset, arg) in args[fixed_len..].iter().enumerate() {
                match_param(variadic, arg, fixed_len + offset, bindings)?;
            }

            Ok(())
        }
        _ if params.len() == args.len() => {
            for (index, (param, arg)) in params.iter().zip(args.iter()).enumerate() {
                match_param(param, arg, index, bindings)?;
            }
            Ok(())
        }
        _ => Err(SignatureError::ArityMismatch {
            expected: params.len(),
            found: args.len(),
        }),
    }
}

fn match_param(
    param: &ParamType,
    arg: &Type,
    param_index: usize,
    bindings: &mut Bindings,
) -> Result<(), SignatureError> {
    match param {
        ParamType::Type(expr) => match_expr(expr, arg, param_index, bindings),
        ParamType::Variadic(inner) => match_param(inner, arg, param_index, bindings),
    }
}

fn match_expr(
    expr: &TypeExpr,
    arg: &Type,
    param_index: usize,
    bindings: &mut Bindings,
) -> Result<(), SignatureError> {
    match expr {
        TypeExpr::Exact(expected) => {
            if assignability(arg, expected) == Compatibility::Assignable {
                Ok(())
            } else {
                Err(SignatureError::TypeMismatch {
                    param_index,
                    expected: expr.clone(),
                    found: arg.clone(),
                })
            }
        }
        TypeExpr::Generic(name) => bind_generic(name, arg, bindings),
        TypeExpr::Array(inner) => match arg {
            Type::Array(arg_inner, _) => match_expr(inner, arg_inner, param_index, bindings),
            _ => Err(SignatureError::TypeMismatch {
                param_index,
                expected: expr.clone(),
                found: arg.clone(),
            }),
        },
        TypeExpr::Optional(inner) => match arg {
            Type::Optional(arg_inner) => match_expr(inner, arg_inner, param_index, bindings),
            Type::None => Ok(()),
            _ => Err(SignatureError::TypeMismatch {
                param_index,
                expected: expr.clone(),
                found: arg.clone(),
            }),
        },
        TypeExpr::OneOf(candidates) => {
            let mut unknown_seen = false;
            for candidate in candidates {
                match match_expr(candidate, arg, param_index, &mut bindings.clone()) {
                    Ok(()) => return match_expr(candidate, arg, param_index, bindings),
                    Err(SignatureError::TypeMismatch {
                        found: Type::Unknown(_),
                        ..
                    }) => unknown_seen = true,
                    Err(_) => {}
                }
            }

            if unknown_seen {
                Ok(())
            } else {
                Err(SignatureError::TypeMismatch {
                    param_index,
                    expected: expr.clone(),
                    found: arg.clone(),
                })
            }
        }
    }
}

fn bind_generic(name: &str, arg: &Type, bindings: &mut Bindings) -> Result<(), SignatureError> {
    if let Some(existing) = bindings.get(name) {
        if existing == arg {
            Ok(())
        } else {
            Err(SignatureError::ConflictingGenericBinding {
                generic: name.to_string(),
                existing: existing.clone(),
                found: arg.clone(),
            })
        }
    } else {
        bindings.insert(name.to_string(), arg.clone());
        Ok(())
    }
}

fn resolve_expr(expr: &TypeExpr, bindings: &Bindings) -> Result<Type, SignatureError> {
    match expr {
        TypeExpr::Exact(ty) => Ok(ty.clone()),
        TypeExpr::Generic(name) => {
            bindings
                .get(name)
                .cloned()
                .ok_or_else(|| SignatureError::UnboundGeneric {
                    generic: name.clone(),
                })
        }
        TypeExpr::Array(inner) => Ok(Type::Array(Box::new(resolve_expr(inner, bindings)?), None)),
        TypeExpr::Optional(inner) => Ok(Type::Optional(Box::new(resolve_expr(inner, bindings)?))),
        TypeExpr::OneOf(candidates) => candidates
            .first()
            .map(|candidate| resolve_expr(candidate, bindings))
            .unwrap_or_else(|| Ok(Type::Never)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NumberKind;

    fn generic(name: &str) -> TypeExpr {
        TypeExpr::Generic(name.to_string())
    }

    fn array_of(expr: TypeExpr) -> TypeExpr {
        TypeExpr::Array(Box::new(expr))
    }

    fn optional(expr: TypeExpr) -> TypeExpr {
        TypeExpr::Optional(Box::new(expr))
    }

    fn array_type(ty: Type) -> Type {
        Type::Array(Box::new(ty), None)
    }

    #[test]
    fn array_len_binds_element_generic_and_returns_int() {
        let signature = FunctionSig {
            name: "array::len".into(),
            generics: vec![GenericParam::new("T")],
            params: vec![ParamType::Type(array_of(generic("T")))],
            returns: TypeExpr::Exact(Type::Number(NumberKind::Int)),
        };

        let matched =
            match_signature(&signature, &[array_type(Type::String)]).expect("signature matches");

        assert_eq!(matched.result_type, Type::Number(NumberKind::Int));
    }

    #[test]
    fn array_first_reuses_bound_element_generic_in_optional_return() {
        let signature = FunctionSig {
            name: "array::first".into(),
            generics: vec![GenericParam::new("T")],
            params: vec![ParamType::Type(array_of(generic("T")))],
            returns: optional(generic("T")),
        };

        let matched =
            match_signature(&signature, &[array_type(Type::String)]).expect("signature matches");

        assert_eq!(matched.result_type, Type::Optional(Box::new(Type::String)));
    }

    #[test]
    fn repeated_generic_parameters_must_be_compatible() {
        let signature = FunctionSig {
            name: "array::append".into(),
            generics: vec![GenericParam::new("T")],
            params: vec![
                ParamType::Type(array_of(generic("T"))),
                ParamType::Type(generic("T")),
            ],
            returns: array_of(generic("T")),
        };

        let matched = match_signature(&signature, &[array_type(Type::String), Type::String])
            .expect("signature matches");

        assert_eq!(matched.result_type, array_type(Type::String));
    }

    #[test]
    fn conflicting_generic_bindings_are_reported() {
        let signature = FunctionSig {
            name: "array::append".into(),
            generics: vec![GenericParam::new("T")],
            params: vec![
                ParamType::Type(array_of(generic("T"))),
                ParamType::Type(generic("T")),
            ],
            returns: array_of(generic("T")),
        };

        let error = match_signature(&signature, &[array_type(Type::String), Type::Bool])
            .expect_err("signature should reject conflicting generic binding");

        assert_eq!(
            error,
            SignatureError::ConflictingGenericBinding {
                generic: "T".into(),
                existing: Type::String,
                found: Type::Bool,
            }
        );
    }

    #[test]
    fn exact_parameters_use_assignability_rules() {
        let signature = FunctionSig {
            name: "math::abs".into(),
            generics: vec![],
            params: vec![ParamType::Type(TypeExpr::Exact(Type::Number(
                NumberKind::Number,
            )))],
            returns: TypeExpr::Exact(Type::Number(NumberKind::Number)),
        };

        let int_literal = Type::Literal(crate::LiteralType::Int(5));
        let matched = match_signature(&signature, &[int_literal]).expect("literal int is numeric");

        assert_eq!(matched.result_type, Type::Number(NumberKind::Number));
    }

    #[test]
    fn exact_parameters_report_type_mismatch() {
        let signature = FunctionSig {
            name: "math::abs".into(),
            generics: vec![],
            params: vec![ParamType::Type(TypeExpr::Exact(Type::Number(
                NumberKind::Number,
            )))],
            returns: TypeExpr::Exact(Type::Number(NumberKind::Number)),
        };

        let error =
            match_signature(&signature, &[Type::String]).expect_err("string is not numeric");

        assert_eq!(
            error,
            SignatureError::TypeMismatch {
                param_index: 0,
                expected: TypeExpr::Exact(Type::Number(NumberKind::Number)),
                found: Type::String,
            }
        );
    }
}
