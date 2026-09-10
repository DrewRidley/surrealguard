//! `DEFINE` statement analysis dispatch.
//!
//! Fans out over the lowered [`ast::DefineStmt`] to one analyzer per
//! Tier 1 DEFINE kind. `Other` covers the unmodeled long tail
//! (ACCESS/API/BUCKET/CONFIG/...) explicitly.
//!
//! The one contract every modeled kind shares lives here: a definition does
//! not silently redefine (1022). `OVERWRITE` states the intent to replace and
//! `IF NOT EXISTS` the intent to keep the first, so neither is a duplicate.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;
use surrealguard_syntax::span::{ByteRange, SourceSpan};

use crate::analyzer::context::AnalysisContext;

pub mod analyzer;
pub mod event;
pub mod field;
pub mod function;
pub mod index;
pub mod param;
pub mod permissions;
pub mod table;

pub(crate) fn analyze_define(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineStmt) -> Kind {
    match stmt {
        ast::DefineStmt::Table(def) => table::analyze_define_table(ctx, def),
        ast::DefineStmt::Field(def) => field::analyze_define_field(ctx, def),
        ast::DefineStmt::Index(def) => index::analyze_define_index(ctx, def),
        ast::DefineStmt::Event(def) => event::analyze_define_event(ctx, def),
        ast::DefineStmt::Param(def) => param::analyze_define_param(ctx, def),
        ast::DefineStmt::Function(def) => function::analyze_define_function(ctx, def),
        ast::DefineStmt::Analyzer(def) => analyzer::analyze_define_analyzer(ctx, def),
        ast::DefineStmt::Other(_) => Kind::Any,
    }
}

/// Emits the duplicate-definition finding (1022) for a `DEFINE` that names
/// something already in the catalog without `OVERWRITE`/`IF NOT EXISTS`.
///
/// `subject` is the rendered thing (`` `person` ``, `` `idx` on `person` ``),
/// `redefine` the statement head that would make the replacement deliberate
/// (`DEFINE INDEX OVERWRITE idx ON person`), `existing` where it was first
/// defined.
pub(crate) fn emit_duplicate_definition(
    ctx: &mut AnalysisContext<'_>,
    name_span: ByteRange,
    subject: &str,
    redefine: &str,
    existing: SourceSpan,
) {
    let span = SourceSpan::new(ctx.source().clone(), name_span);
    ctx.emit(
        surrealguard_diagnostics::catalog::finding(
            span,
            1022,
            format!("{subject} is already defined; this DEFINE silently replaces the earlier one"),
        )
        .with_help(format!("use `{redefine}` to redefine it intentionally"))
        .with_related(existing, format!("{subject} is defined here")),
    );
}
