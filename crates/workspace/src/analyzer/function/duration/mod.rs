//! `duration` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

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

/// Every `duration::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "duration::days",
        "The whole days in a duration.",
        days::signature,
        days::analyze_duration_days,
    ),
    BuiltinEntry::new(
        "duration::from_days",
        "A duration of the given number of days.",
        from_days::signature,
        from_days::analyze_duration_from_days,
    ),
    BuiltinEntry::new(
        "duration::from_hours",
        "A duration of the given number of hours.",
        from_hours::signature,
        from_hours::analyze_duration_from_hours,
    ),
    BuiltinEntry::new(
        "duration::from_micros",
        "A duration of the given number of microseconds.",
        from_micros::signature,
        from_micros::analyze_duration_from_micros,
    ),
    BuiltinEntry::new(
        "duration::from_millis",
        "A duration of the given number of milliseconds.",
        from_millis::signature,
        from_millis::analyze_duration_from_millis,
    ),
    BuiltinEntry::new(
        "duration::from_mins",
        "A duration of the given number of minutes.",
        from_mins::signature,
        from_mins::analyze_duration_from_mins,
    ),
    BuiltinEntry::new(
        "duration::from_nanos",
        "A duration of the given number of nanoseconds.",
        from_nanos::signature,
        from_nanos::analyze_duration_from_nanos,
    ),
    BuiltinEntry::new(
        "duration::from_secs",
        "A duration of the given number of seconds.",
        from_secs::signature,
        from_secs::analyze_duration_from_secs,
    ),
    BuiltinEntry::new(
        "duration::from_weeks",
        "A duration of the given number of weeks.",
        from_weeks::signature,
        from_weeks::analyze_duration_from_weeks,
    ),
    BuiltinEntry::new(
        "duration::hours",
        "The whole hours in a duration.",
        hours::signature,
        hours::analyze_duration_hours,
    ),
    BuiltinEntry::new(
        "duration::micros",
        "The whole microseconds in a duration.",
        micros::signature,
        micros::analyze_duration_micros,
    ),
    BuiltinEntry::new(
        "duration::millis",
        "The whole milliseconds in a duration.",
        millis::signature,
        millis::analyze_duration_millis,
    ),
    BuiltinEntry::new(
        "duration::mins",
        "The whole minutes in a duration.",
        mins::signature,
        mins::analyze_duration_mins,
    ),
    BuiltinEntry::new(
        "duration::nanos",
        "The nanoseconds in a duration.",
        nanos::signature,
        nanos::analyze_duration_nanos,
    ),
    BuiltinEntry::new(
        "duration::secs",
        "The whole seconds in a duration.",
        secs::signature,
        secs::analyze_duration_secs,
    ),
    BuiltinEntry::new(
        "duration::weeks",
        "The whole weeks in a duration.",
        weeks::signature,
        weeks::analyze_duration_weeks,
    ),
    BuiltinEntry::new(
        "duration::years",
        "The whole years in a duration.",
        years::signature,
        years::analyze_duration_years,
    ),
];
