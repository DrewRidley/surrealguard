//! `duration` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod days;
pub mod from_days;
pub mod from_hours;
pub mod from_micros;
pub mod from_millis;
pub mod from_mins;
pub mod from_nanos;
pub mod from_secs;
pub mod from_weeks;
pub mod hours;
pub mod micros;
pub mod millis;
pub mod mins;
pub mod nanos;
pub mod secs;
pub mod weeks;
pub mod years;

pub(crate) fn analyze_duration_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "duration::days" => days::analyze_duration_days(ctx, call, args),
        "duration::from_days" => from_days::analyze_duration_from_days(ctx, call, args),
        "duration::from_hours" => from_hours::analyze_duration_from_hours(ctx, call, args),
        "duration::from_micros" => from_micros::analyze_duration_from_micros(ctx, call, args),
        "duration::from_millis" => from_millis::analyze_duration_from_millis(ctx, call, args),
        "duration::from_mins" => from_mins::analyze_duration_from_mins(ctx, call, args),
        "duration::from_nanos" => from_nanos::analyze_duration_from_nanos(ctx, call, args),
        "duration::from_secs" => from_secs::analyze_duration_from_secs(ctx, call, args),
        "duration::from_weeks" => from_weeks::analyze_duration_from_weeks(ctx, call, args),
        "duration::hours" => hours::analyze_duration_hours(ctx, call, args),
        "duration::micros" => micros::analyze_duration_micros(ctx, call, args),
        "duration::millis" => millis::analyze_duration_millis(ctx, call, args),
        "duration::mins" => mins::analyze_duration_mins(ctx, call, args),
        "duration::nanos" => nanos::analyze_duration_nanos(ctx, call, args),
        "duration::secs" => secs::analyze_duration_secs(ctx, call, args),
        "duration::weeks" => weeks::analyze_duration_weeks(ctx, call, args),
        "duration::years" => years::analyze_duration_years(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
