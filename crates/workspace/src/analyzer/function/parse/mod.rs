//! `parse` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod email_host;
pub mod email_user;
pub mod url_domain;
pub mod url_fragment;
pub mod url_host;
pub mod url_path;
pub mod url_port;
pub mod url_query;
pub mod url_scheme;

/// Every `parse::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "parse::email::host",
        "The host part of an email address.",
        email_host::signature,
        email_host::analyze_parse_email_host,
    ),
    BuiltinEntry::new(
        "parse::email::user",
        "The user part of an email address.",
        email_user::signature,
        email_user::analyze_parse_email_user,
    ),
    BuiltinEntry::new(
        "parse::url::domain",
        "The domain of a URL.",
        url_domain::signature,
        url_domain::analyze_parse_url_domain,
    ),
    BuiltinEntry::new(
        "parse::url::fragment",
        "The fragment of a URL, or NONE.",
        url_fragment::signature,
        url_fragment::analyze_parse_url_fragment,
    ),
    BuiltinEntry::new(
        "parse::url::host",
        "The host of a URL.",
        url_host::signature,
        url_host::analyze_parse_url_host,
    ),
    BuiltinEntry::new(
        "parse::url::path",
        "The path of a URL.",
        url_path::signature,
        url_path::analyze_parse_url_path,
    ),
    BuiltinEntry::new(
        "parse::url::port",
        "The port of a URL, or NONE.",
        url_port::signature,
        url_port::analyze_parse_url_port,
    ),
    BuiltinEntry::new(
        "parse::url::query",
        "The query string of a URL, or NONE.",
        url_query::signature,
        url_query::analyze_parse_url_query,
    ),
    BuiltinEntry::new(
        "parse::url::scheme",
        "The scheme of a URL.",
        url_scheme::signature,
        url_scheme::analyze_parse_url_scheme,
    ),
];
