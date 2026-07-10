//! `DEFINE PARAM` analysis.
//!
//! Contract: the definition gives `$name` a database-side default, so
//! later reads are neither unknown nor host-required — they carry the
//! default's kind unless the host overrides.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub fn analyze_define_param(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineParam) -> Kind {
    if let Some(value) = &stmt.value {
        let fact = crate::analyzer::expression::expr_fact(ctx, value);
        ctx.define_param_default(stmt.name.node.clone(), fact);
    }
    Kind::None
}
