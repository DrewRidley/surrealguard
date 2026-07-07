//! `file` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod bucket;
pub mod copy;
pub mod copy_if_not_exists;
pub mod delete;
pub mod exists;
pub mod get;
pub mod head;
pub mod key;
pub mod list;
pub mod put;
pub mod put_if_not_exists;
pub mod rename;
pub mod rename_if_not_exists;

pub fn analyze_file_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "file::bucket" => bucket::analyze_file_bucket(ctx, call, args),
        "file::copy" => copy::analyze_file_copy(ctx, call, args),
        "file::copy_if_not_exists" => {
            copy_if_not_exists::analyze_file_copy_if_not_exists(ctx, call, args)
        }
        "file::delete" => delete::analyze_file_delete(ctx, call, args),
        "file::exists" => exists::analyze_file_exists(ctx, call, args),
        "file::get" => get::analyze_file_get(ctx, call, args),
        "file::head" => head::analyze_file_head(ctx, call, args),
        "file::key" => key::analyze_file_key(ctx, call, args),
        "file::list" => list::analyze_file_list(ctx, call, args),
        "file::put" => put::analyze_file_put(ctx, call, args),
        "file::put_if_not_exists" => {
            put_if_not_exists::analyze_file_put_if_not_exists(ctx, call, args)
        }
        "file::rename" => rename::analyze_file_rename(ctx, call, args),
        "file::rename_if_not_exists" => {
            rename_if_not_exists::analyze_file_rename_if_not_exists(ctx, call, args)
        }
        _ => Kind::Any,
    }
}
