//! `file::rename` function analysis: `file::rename(file, target) -> any`.
//!
//! Renames/moves the file to `target`. Result kind is a side-effect status not
//! stable enough to commit to, so `any` is inferred; the (file, target) arity
//! is still enforced.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;
use crate::analyzer::function::signature::{apply, ParamKind, ReturnKind, Signature};

pub fn analyze_file_rename(ctx: &mut AnalysisContext<'_>, call: &ast::Call, args: &[Kind]) -> Kind {
    apply(
        ctx,
        call,
        &Signature {
            min_args: 2,
            max_args: Some(2),
            arg_kinds: vec![ParamKind::Any, ParamKind::Any],
            return_kind: ReturnKind::Fixed(Kind::None),
        },
        args,
    )
}
