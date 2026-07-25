//! `time` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod ceil;
pub mod day;
pub mod floor;
pub mod format;
pub mod from_micros;
pub mod from_millis;
pub mod from_nanos;
pub mod from_secs;
pub mod from_ulid;
pub mod from_unix;
pub mod from_uuid;
pub mod group;
pub mod hour;
pub mod is_leap_year;
pub mod max;
pub mod micros;
pub mod millis;
pub mod min;
pub mod minute;
pub mod month;
pub mod nano;
pub mod now;
pub mod round;
pub mod second;
pub mod set_day;
pub mod set_hour;
pub mod set_minute;
pub mod set_month;
pub mod set_nanosecond;
pub mod set_second;
pub mod set_year;
pub mod timezone;
pub mod unix;
pub mod wday;
pub mod week;
pub mod yday;
pub mod year;

pub(crate) fn analyze_time_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "time::ceil" => ceil::analyze_time_ceil(ctx, call, args),
        "time::day" => day::analyze_time_day(ctx, call, args),
        "time::floor" => floor::analyze_time_floor(ctx, call, args),
        "time::format" => format::analyze_time_format(ctx, call, args),
        "time::from_micros" | "time::from::micros" => {
            from_micros::analyze_time_from_micros(ctx, call, args)
        }
        "time::from_millis" | "time::from::millis" => {
            from_millis::analyze_time_from_millis(ctx, call, args)
        }
        "time::from_nanos" | "time::from::nanos" => {
            from_nanos::analyze_time_from_nanos(ctx, call, args)
        }
        "time::from_secs" | "time::from::secs" => {
            from_secs::analyze_time_from_secs(ctx, call, args)
        }
        "time::from_ulid" => from_ulid::analyze_time_from_ulid(ctx, call, args),
        "time::from_unix" | "time::from::unix" => {
            from_unix::analyze_time_from_unix(ctx, call, args)
        }
        "time::from_uuid" => from_uuid::analyze_time_from_uuid(ctx, call, args),
        "time::group" => group::analyze_time_group(ctx, call, args),
        "time::hour" => hour::analyze_time_hour(ctx, call, args),
        "time::is_leap_year" => is_leap_year::analyze_time_is_leap_year(ctx, call, args),
        "time::max" => max::analyze_time_max(ctx, call, args),
        "time::micros" => micros::analyze_time_micros(ctx, call, args),
        "time::millis" => millis::analyze_time_millis(ctx, call, args),
        "time::min" => min::analyze_time_min(ctx, call, args),
        "time::minute" => minute::analyze_time_minute(ctx, call, args),
        "time::month" => month::analyze_time_month(ctx, call, args),
        "time::nano" => nano::analyze_time_nano(ctx, call, args),
        "time::now" => now::analyze_time_now(ctx, call, args),
        "time::round" => round::analyze_time_round(ctx, call, args),
        "time::second" => second::analyze_time_second(ctx, call, args),
        "time::set_day" => set_day::analyze_time_set_day(ctx, call, args),
        "time::set_hour" => set_hour::analyze_time_set_hour(ctx, call, args),
        "time::set_minute" => set_minute::analyze_time_set_minute(ctx, call, args),
        "time::set_month" => set_month::analyze_time_set_month(ctx, call, args),
        "time::set_nanosecond" => set_nanosecond::analyze_time_set_nanosecond(ctx, call, args),
        "time::set_second" => set_second::analyze_time_set_second(ctx, call, args),
        "time::set_year" => set_year::analyze_time_set_year(ctx, call, args),
        "time::timezone" => timezone::analyze_time_timezone(ctx, call, args),
        "time::unix" => unix::analyze_time_unix(ctx, call, args),
        "time::wday" => wday::analyze_time_wday(ctx, call, args),
        "time::week" => week::analyze_time_week(ctx, call, args),
        "time::yday" => yday::analyze_time_yday(ctx, call, args),
        "time::year" => year::analyze_time_year(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
