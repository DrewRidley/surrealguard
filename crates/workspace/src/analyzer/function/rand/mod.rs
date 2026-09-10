//! `rand` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod bool;
pub mod duration;
pub mod r#enum;
pub mod float;
pub mod id;
pub mod int;
pub mod string;
pub mod time;
pub mod ulid;
// The bare `rand()` builtin has no path segment after its family, so the
// file mirroring it is `rand/rand.rs` — the naming rule every other leaf
// follows, not an accidentally nested module.
#[allow(clippy::module_inception)]
pub mod rand;
pub mod uuid;
pub mod uuid_v4;
pub mod uuid_v7;

/// Every `rand::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "rand::bool",
        "A random boolean.",
        bool::signature,
        bool::analyze_rand_bool,
    ),
    BuiltinEntry::new(
        "rand::duration",
        "A random duration between two bounds.",
        duration::signature,
        duration::analyze_rand_duration,
    ),
    BuiltinEntry::new(
        "rand::enum",
        "One of the given values, chosen at random.",
        r#enum::signature,
        r#enum::analyze_rand_enum,
    ),
    BuiltinEntry::new(
        "rand::float",
        "A random float, optionally between two bounds.",
        float::signature,
        float::analyze_rand_float,
    ),
    BuiltinEntry::new(
        "rand::id",
        "A random record id, or one for the given table.",
        id::signature,
        id::analyze_rand_id,
    ),
    BuiltinEntry::new(
        "rand::int",
        "A random integer, optionally between two bounds.",
        int::signature,
        int::analyze_rand_int,
    ),
    BuiltinEntry::new(
        "rand::string",
        "A random string, of an optional length or length range.",
        string::signature,
        string::analyze_rand_string,
    ),
    BuiltinEntry::new(
        "rand::time",
        "A random datetime, optionally between two bounds.",
        time::signature,
        time::analyze_rand_time,
    ),
    BuiltinEntry::new(
        "rand::ulid",
        "A random ULID, optionally seeded from a datetime.",
        ulid::signature,
        ulid::analyze_rand_ulid,
    ),
    BuiltinEntry::new(
        "rand::uuid",
        "A random UUID, optionally seeded from a datetime.",
        uuid::signature,
        uuid::analyze_rand_uuid,
    ),
    BuiltinEntry::new(
        "rand",
        "A random float between 0 and 1.",
        rand::signature,
        rand::analyze_rand_rand,
    ),
    BuiltinEntry::new(
        "rand::uuid::v4",
        "A random UUID v4.",
        uuid_v4::signature,
        uuid_v4::analyze_rand_uuid_v4,
    ),
    BuiltinEntry::new(
        "rand::uuid::v7",
        "A random UUID v7, optionally for a datetime.",
        uuid_v7::signature,
        uuid_v7::analyze_rand_uuid_v7,
    ),
];
