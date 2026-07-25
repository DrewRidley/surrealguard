//! `session` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod ac;
pub mod rd;
pub mod db;
pub mod id;
pub mod ip;
pub mod ns;
pub mod origin;
pub mod sc;
pub mod sd;
pub mod token;

pub(crate) fn analyze_session_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "session::ac" => ac::analyze_session_ac(ctx, call, args),
        "session::db" => db::analyze_session_db(ctx, call, args),
        "session::id" => id::analyze_session_id(ctx, call, args),
        "session::ip" => ip::analyze_session_ip(ctx, call, args),
        "session::ns" => ns::analyze_session_ns(ctx, call, args),
        "session::origin" => origin::analyze_session_origin(ctx, call, args),
        "session::rd" => rd::analyze_session_rd(ctx, call, args),
        "session::token" => token::analyze_session_token(ctx, call, args),
        "session::sc" => sc::analyze_session_sc(ctx, call, args),
        "session::sd" => sd::analyze_session_sd(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
