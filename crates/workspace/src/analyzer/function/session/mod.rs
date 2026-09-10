//! `session` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod ac;
pub mod db;
pub mod id;
pub mod ip;
pub mod ns;
pub mod origin;
pub mod rd;
pub mod sc;
pub mod sd;
pub mod token;

/// Every `session::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "session::ac",
        "The access method of the current session.",
        ac::signature,
        ac::analyze_session_ac,
    ),
    BuiltinEntry::new(
        "session::db",
        "The database of the current session.",
        db::signature,
        db::analyze_session_db,
    ),
    BuiltinEntry::new(
        "session::id",
        "The id of the current session.",
        id::signature,
        id::analyze_session_id,
    ),
    BuiltinEntry::new(
        "session::ip",
        "The client IP address of the current session.",
        ip::signature,
        ip::analyze_session_ip,
    ),
    BuiltinEntry::new(
        "session::ns",
        "The namespace of the current session.",
        ns::signature,
        ns::analyze_session_ns,
    ),
    BuiltinEntry::new(
        "session::origin",
        "The origin of the current session.",
        origin::signature,
        origin::analyze_session_origin,
    ),
    BuiltinEntry::new(
        "session::rd",
        "The record id the current session authenticated as.",
        rd::signature,
        rd::analyze_session_rd,
    ),
    BuiltinEntry::new(
        "session::token",
        "The authentication token claims of the current session.",
        token::signature,
        token::analyze_session_token,
    ),
    BuiltinEntry::new(
        "session::sc",
        "The scope of the current session (removed in 2.0).",
        sc::signature,
        sc::analyze_session_sc,
    ),
    BuiltinEntry::new(
        "session::sd",
        "The record id the current session authenticated as (removed in 2.0).",
        sd::signature,
        sd::analyze_session_sd,
    ),
];
