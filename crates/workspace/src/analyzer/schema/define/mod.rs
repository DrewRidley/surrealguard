//! `DEFINE` statement analysis dispatch.
//!
//! Fans out over the lowered [`ast::DefineStmt`] to one analyzer per
//! Tier 1 DEFINE kind. `Other` covers the unmodeled long tail
//! (ACCESS/API/BUCKET/CONFIG/...) explicitly.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod analyzer;
pub mod event;
pub mod field;
pub mod function;
pub mod index;
pub mod param;
pub mod table;

pub fn analyze_define(ctx: &mut AnalysisContext<'_>, stmt: &ast::DefineStmt) -> Kind {
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
