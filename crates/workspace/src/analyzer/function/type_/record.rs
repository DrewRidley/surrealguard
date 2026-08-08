//! `type::record` function analysis: `type::record(any) -> record`,
//! `type::record(table, id) -> record<table>`.
//!
//! The two-argument form names its table as a *value*. When that value is a
//! constant — a string literal, or a `LET` binding tracing back to one — the
//! constructed link's table is decided statically, so the call is a
//! `record<person>` rather than the unconstrained `record` (engine-verified on
//! 3.2.3; see [`super::constant_table_arg`]). That is the whole difference
//! between `SET owner = type::record('person', $id)` being accepted against a
//! `record<person>` field and being a false 2001.
//!
//! A non-constant table argument stays unconstrained: `record`, not `any`, and
//! not the table the surrounding code happens to hope for.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub(crate) fn analyze_type_record(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Kind {
    let tables = constructed_tables(ctx, call, args);
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

/// The tables the constructed record is known to belong to, or the empty
/// vector (the unconstrained `record`) when nothing proves one.
///
/// Two provable shapes, and only two:
///
/// * `type::record(<constant>, id)` — the table half is the constant.
/// * `type::record(x)` where `x` is already a record of known tables —
///   the one-argument form passes a record through unchanged, so its tables
///   are the argument's (`type::record(person:a)` is `person:a`).
///
/// The one-argument *string* form (`type::record('person:a')`) is deliberately
/// not read: recovering the table would mean re-implementing record-id parsing,
/// and a table name is allowed to contain the `:` that the split would key on.
pub(super) fn constructed_tables(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    args: &[Kind],
) -> Vec<surrealdb_types::Table> {
    if call.args.len() >= 2 {
        return super::constant_table_arg(ctx, call, 0)
            .map(|table| vec![table])
            .unwrap_or_default();
    }
    match args.first() {
        Some(Kind::Record(tables)) => tables.clone(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use crate::analysis::{analyze_query, Workspace};

    fn kind_of(query: &str) -> Option<Kind> {
        let mut workspace = Workspace::default();
        analyze_query(&mut workspace, query).response_kind
    }

    use super::*;

    #[test]
    fn a_constant_table_argument_names_the_constructed_table() {
        assert_eq!(
            kind_of("RETURN type::record('person', $id);"),
            Some(Kind::Record(vec!["person".into()]))
        );
        // A `LET`-bound constant is as constant as the literal.
        assert_eq!(
            kind_of("LET $t = 'person';\nRETURN type::record($t, $id);"),
            Some(Kind::Record(vec!["person".into()]))
        );
    }

    #[test]
    fn a_runtime_table_argument_stays_unconstrained() {
        // Prove or stay silent: nothing here says which table, so the answer
        // is `record` — not `any`, and not a guess.
        assert_eq!(
            kind_of("RETURN type::record($table, $id);"),
            Some(Kind::Record(Vec::new()))
        );
    }

    #[test]
    fn the_one_argument_form_passes_a_record_through() {
        assert_eq!(
            kind_of("RETURN type::record(person:one);"),
            Some(Kind::Record(vec!["person".into()]))
        );
        // A string is a record id the engine parses at runtime; the table is
        // not read back out of it here.
        assert_eq!(
            kind_of("RETURN type::record('person:one');"),
            Some(Kind::Record(Vec::new()))
        );
    }
}
