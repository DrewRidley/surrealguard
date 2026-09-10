//! `object` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod entries;
pub mod extend;
pub mod from_entries;
pub mod is_empty;
pub mod keys;
pub mod len;
pub mod remove;
pub mod values;

/// Every `object::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "object::entries",
        "The object's key-value pairs as two-element arrays.",
        entries::signature,
        entries::analyze_object_entries,
    ),
    BuiltinEntry::new(
        "object::extend",
        "The object with another object's fields merged in.",
        extend::signature,
        extend::analyze_object_extend,
    ),
    BuiltinEntry::new(
        "object::from_entries",
        "An object built from an array of key-value pairs.",
        from_entries::signature,
        from_entries::analyze_object_from_entries,
    ),
    BuiltinEntry::new(
        "object::is_empty",
        "Whether the object has no fields.",
        is_empty::signature,
        is_empty::analyze_object_is_empty,
    ),
    BuiltinEntry::new(
        "object::keys",
        "The object's keys.",
        keys::signature,
        keys::analyze_object_keys,
    ),
    BuiltinEntry::new(
        "object::len",
        "The number of fields in the object.",
        len::signature,
        len::analyze_object_len,
    ),
    BuiltinEntry::new(
        "object::remove",
        "The object without the given key or keys.",
        remove::signature,
        remove::analyze_object_remove,
    ),
    BuiltinEntry::new(
        "object::values",
        "The object's values.",
        values::signature,
        values::analyze_object_values,
    ),
];
