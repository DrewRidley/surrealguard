//! `parse` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod email_host;
pub mod email_user;
pub mod url_domain;
pub mod url_fragment;
pub mod url_host;
pub mod url_path;
pub mod url_port;
pub mod url_query;
pub mod url_scheme;

pub fn analyze_parse_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "parse::email::host" => email_host::analyze_parse_email_host(ctx, call, args),
        "parse::email::user" => email_user::analyze_parse_email_user(ctx, call, args),
        "parse::url::domain" => url_domain::analyze_parse_url_domain(ctx, call, args),
        "parse::url::fragment" => url_fragment::analyze_parse_url_fragment(ctx, call, args),
        "parse::url::host" => url_host::analyze_parse_url_host(ctx, call, args),
        "parse::url::path" => url_path::analyze_parse_url_path(ctx, call, args),
        "parse::url::port" => url_port::analyze_parse_url_port(ctx, call, args),
        "parse::url::query" => url_query::analyze_parse_url_query(ctx, call, args),
        "parse::url::scheme" => url_scheme::analyze_parse_url_scheme(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
