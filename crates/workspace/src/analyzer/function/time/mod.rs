//! `time` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

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

/// Every `time::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "time::ceil",
        "The datetime rounded up to a multiple of the duration.",
        ceil::signature,
        ceil::analyze_time_ceil,
    ),
    BuiltinEntry::new(
        "time::day",
        "The day of the month of a datetime, or of now.",
        day::signature,
        day::analyze_time_day,
    ),
    BuiltinEntry::new(
        "time::floor",
        "The datetime rounded down to a multiple of the duration.",
        floor::signature,
        floor::analyze_time_floor,
    ),
    BuiltinEntry::new(
        "time::format",
        "The datetime formatted with a strftime-style pattern.",
        format::signature,
        format::analyze_time_format,
    ),
    BuiltinEntry::new(
        "time::from_micros",
        "The datetime for a Unix timestamp in microseconds.",
        from_micros::signature,
        from_micros::analyze_time_from_micros,
    ),
    BuiltinEntry::new(
        "time::from_millis",
        "The datetime for a Unix timestamp in milliseconds.",
        from_millis::signature,
        from_millis::analyze_time_from_millis,
    ),
    BuiltinEntry::new(
        "time::from_nanos",
        "The datetime for a Unix timestamp in nanoseconds.",
        from_nanos::signature,
        from_nanos::analyze_time_from_nanos,
    ),
    BuiltinEntry::new(
        "time::from_secs",
        "The datetime for a Unix timestamp in seconds.",
        from_secs::signature,
        from_secs::analyze_time_from_secs,
    ),
    BuiltinEntry::new(
        "time::from_ulid",
        "The datetime embedded in a ULID.",
        from_ulid::signature,
        from_ulid::analyze_time_from_ulid,
    ),
    BuiltinEntry::new(
        "time::from_unix",
        "The datetime for a Unix timestamp in seconds.",
        from_unix::signature,
        from_unix::analyze_time_from_unix,
    ),
    BuiltinEntry::new(
        "time::from_uuid",
        "The datetime embedded in a UUID v7.",
        from_uuid::signature,
        from_uuid::analyze_time_from_uuid,
    ),
    BuiltinEntry::new(
        "time::group",
        "The datetime truncated to the given unit (`year`, `month`, `day`, ...).",
        group::signature,
        group::analyze_time_group,
    ),
    BuiltinEntry::new(
        "time::hour",
        "The hour of a datetime, or of now.",
        hour::signature,
        hour::analyze_time_hour,
    ),
    BuiltinEntry::new(
        "time::is_leap_year",
        "Whether the datetime falls in a leap year.",
        is_leap_year::signature,
        is_leap_year::analyze_time_is_leap_year,
    ),
    BuiltinEntry::new(
        "time::max",
        "The latest datetime in an array.",
        max::signature,
        max::analyze_time_max,
    ),
    BuiltinEntry::new(
        "time::micros",
        "The Unix timestamp of a datetime in microseconds.",
        micros::signature,
        micros::analyze_time_micros,
    ),
    BuiltinEntry::new(
        "time::millis",
        "The Unix timestamp of a datetime in milliseconds.",
        millis::signature,
        millis::analyze_time_millis,
    ),
    BuiltinEntry::new(
        "time::min",
        "The earliest datetime in an array.",
        min::signature,
        min::analyze_time_min,
    ),
    BuiltinEntry::new(
        "time::minute",
        "The minute of a datetime, or of now.",
        minute::signature,
        minute::analyze_time_minute,
    ),
    BuiltinEntry::new(
        "time::month",
        "The month of a datetime, or of now.",
        month::signature,
        month::analyze_time_month,
    ),
    BuiltinEntry::new(
        "time::nano",
        "The Unix timestamp of a datetime in nanoseconds.",
        nano::signature,
        nano::analyze_time_nano,
    ),
    BuiltinEntry::new(
        "time::now",
        "The current datetime.",
        now::signature,
        now::analyze_time_now,
    ),
    BuiltinEntry::new(
        "time::round",
        "The datetime rounded to the nearest multiple of the duration.",
        round::signature,
        round::analyze_time_round,
    ),
    BuiltinEntry::new(
        "time::second",
        "The second of a datetime, or of now.",
        second::signature,
        second::analyze_time_second,
    ),
    BuiltinEntry::new(
        "time::set_day",
        "The datetime with its day of the month replaced.",
        set_day::signature,
        set_day::analyze_time_set_day,
    ),
    BuiltinEntry::new(
        "time::set_hour",
        "The datetime with its hour replaced.",
        set_hour::signature,
        set_hour::analyze_time_set_hour,
    ),
    BuiltinEntry::new(
        "time::set_minute",
        "The datetime with its minute replaced.",
        set_minute::signature,
        set_minute::analyze_time_set_minute,
    ),
    BuiltinEntry::new(
        "time::set_month",
        "The datetime with its month replaced.",
        set_month::signature,
        set_month::analyze_time_set_month,
    ),
    BuiltinEntry::new(
        "time::set_nanosecond",
        "The datetime with its nanosecond replaced.",
        set_nanosecond::signature,
        set_nanosecond::analyze_time_set_nanosecond,
    ),
    BuiltinEntry::new(
        "time::set_second",
        "The datetime with its second replaced.",
        set_second::signature,
        set_second::analyze_time_set_second,
    ),
    BuiltinEntry::new(
        "time::set_year",
        "The datetime with its year replaced.",
        set_year::signature,
        set_year::analyze_time_set_year,
    ),
    BuiltinEntry::new(
        "time::timezone",
        "The local timezone of the server.",
        timezone::signature,
        timezone::analyze_time_timezone,
    ),
    BuiltinEntry::new(
        "time::unix",
        "The Unix timestamp of a datetime in seconds.",
        unix::signature,
        unix::analyze_time_unix,
    ),
    BuiltinEntry::new(
        "time::wday",
        "The day of the week of a datetime, or of now.",
        wday::signature,
        wday::analyze_time_wday,
    ),
    BuiltinEntry::new(
        "time::week",
        "The ISO week of a datetime, or of now.",
        week::signature,
        week::analyze_time_week,
    ),
    BuiltinEntry::new(
        "time::yday",
        "The day of the year of a datetime, or of now.",
        yday::signature,
        yday::analyze_time_yday,
    ),
    BuiltinEntry::new(
        "time::year",
        "The year of a datetime, or of now.",
        year::signature,
        year::analyze_time_year,
    ),
];
