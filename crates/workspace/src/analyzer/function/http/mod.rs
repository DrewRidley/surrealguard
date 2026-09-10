//! `http` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod delete;
pub mod get;
pub mod head;
pub mod patch;
pub mod post;
pub mod put;

/// Every `http::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "http::delete",
        "Performs an HTTP DELETE request, with optional headers.",
        delete::signature,
        delete::analyze_http_delete,
    ),
    BuiltinEntry::new(
        "http::get",
        "Performs an HTTP GET request, with optional headers.",
        get::signature,
        get::analyze_http_get,
    ),
    BuiltinEntry::new(
        "http::head",
        "Performs an HTTP HEAD request, with optional headers.",
        head::signature,
        head::analyze_http_head,
    ),
    BuiltinEntry::new(
        "http::patch",
        "Performs an HTTP PATCH request with a body, with optional headers.",
        patch::signature,
        patch::analyze_http_patch,
    ),
    BuiltinEntry::new(
        "http::post",
        "Performs an HTTP POST request with a body, with optional headers.",
        post::signature,
        post::analyze_http_post,
    ),
    BuiltinEntry::new(
        "http::put",
        "Performs an HTTP PUT request with a body, with optional headers.",
        put::signature,
        put::analyze_http_put,
    ),
];
