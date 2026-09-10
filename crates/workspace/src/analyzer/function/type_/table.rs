//! `type::table` function analysis: `type::table(any) -> table`.
//!
//! The result is a table *value*, not a string — `type::of(type::table('person'))`
//! is `'table'` on 3.2.3 — so a constant argument makes this the same kind a
//! bare table name in the source infers (`Kind::Table(vec![person])`), and a
//! runtime argument leaves the table unconstrained.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

/// The signature calls are checked against.
pub(crate) fn signature() -> Signature {
    Signature {
        min_args: 1,
        max_args: Some(1),
        arg_kinds: vec![ParamKind::Any],
        return_kind: ReturnKind::Fixed(Kind::Table(Vec::new())),
    }
}

pub(crate) fn analyze_type_table(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let tables = super::constant_table_arg(ctx, call, 0)
        .map(|table| vec![table])
        .unwrap_or_default();
    let mut signature = signature();
    signature.return_kind = ReturnKind::Fixed(Kind::Table(tables));
    apply(ctx, call, &signature, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{analyze_query, Workspace};

    fn kind_of(query: &str) -> Option<Kind> {
        let mut workspace = Workspace::default();
        analyze_query(&mut workspace, query).response_kind
    }

    #[test]
    fn a_constant_argument_names_the_table() {
        assert_eq!(
            kind_of("RETURN type::table('person');"),
            Some(Kind::Table(vec!["person".into()]))
        );
    }

    #[test]
    fn a_runtime_argument_stays_unconstrained() {
        assert_eq!(
            kind_of("RETURN type::table($name);"),
            Some(Kind::Table(Vec::new()))
        );
    }
}
