//! `type::thing` function analysis: `type::thing(table, id) -> record<table>`.
//!
//! `type::thing` is the 2.x spelling of [`super::record`] — SurrealDB 3.2.3
//! removed it ("Invalid function/constant path, did you maybe mean
//! `type::record`"), so a query using it is a 2.x query and means exactly what
//! `type::record` means. It therefore reads its table the same way: a constant
//! table argument names the constructed record's table, anything else leaves it
//! unconstrained.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_type_thing(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let tables = super::record::constructed_tables(ctx, call, args);
    apply(
        ctx,
        call,
        &Signature {
            min_args: 1,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Any],
            return_kind: ReturnKind::Fixed(Kind::Record(tables)),
        },
        args,
    )
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
    fn reads_its_table_exactly_as_type_record_does() {
        assert_eq!(
            kind_of("RETURN type::thing('person', $id);"),
            Some(Kind::Record(vec!["person".into()]))
        );
        assert_eq!(
            kind_of("RETURN type::thing($table, $id);"),
            Some(Kind::Record(Vec::new()))
        );
    }
}
