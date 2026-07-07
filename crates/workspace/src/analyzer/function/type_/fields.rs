//! `type::fields` function analysis: `type::fields(paths) -> field values`.
//!
//! When the paths argument is statically known (an array literal or a
//! binding tracing back to one), each path resolves against the row
//! context and the result is the tuple of their kinds.

use surrealdb_types::{Kind, KindLiteral, Value};
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::const_value_arg;
use crate::analyzer::function::type_::field::field_kind_for_path;

pub fn analyze_type_fields(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    let _ = args;
    let Some(Value::Array(paths)) = const_value_arg(ctx, call, 0) else {
        return Kind::Any;
    };

    let mut kinds = Vec::new();
    for path in paths.iter() {
        let Value::String(path) = path else {
            return Kind::Any;
        };
        match field_kind_for_path(ctx, path) {
            Some(kind) => kinds.push(kind),
            None => return Kind::Any,
        }
    }
    Kind::Literal(KindLiteral::Array(kinds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::test_support;

    #[test]
    fn non_constant_paths_stay_unknown() {
        test_support::with_ctx(|ctx| {
            let call = test_support::synthetic_call("type::fields");
            assert_eq!(analyze_type_fields(ctx, &call, &[Kind::Any]), Kind::Any);
        })
    }
}
