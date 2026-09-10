//! `api` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod invoke;
pub mod req_body;
pub mod req_max_body;
pub mod res_body;
pub mod res_header;
pub mod res_headers;
pub mod res_status;
pub mod timeout;

/// Every `api::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "api::invoke",
        "Invokes a user-defined API route at the given path, with optional request options.",
        invoke::signature,
        invoke::analyze_api_invoke,
    ),
    BuiltinEntry::new(
        "api::req::body",
        "Middleware: parses the raw request body before the handler runs.",
        req_body::signature,
        req_body::analyze_api_req_body,
    ),
    BuiltinEntry::new(
        "api::req::max_body",
        "Middleware: caps the size of the raw request body, as a byte count or a size string such as `'512kb'`.",
        req_max_body::signature,
        req_max_body::analyze_api_req_max_body,
    ),
    BuiltinEntry::new(
        "api::res::body",
        "Middleware: sets the response body.",
        res_body::signature,
        res_body::analyze_api_res_body,
    ),
    BuiltinEntry::new(
        "api::res::header",
        "Middleware: sets one response header.",
        res_header::signature,
        res_header::analyze_api_res_header,
    ),
    BuiltinEntry::new(
        "api::res::headers",
        "Middleware: sets several response headers from an object.",
        res_headers::signature,
        res_headers::analyze_api_res_headers,
    ),
    BuiltinEntry::new(
        "api::res::status",
        "Middleware: sets the response status code.",
        res_status::signature,
        res_status::analyze_api_res_status,
    ),
    BuiltinEntry::new(
        "api::timeout",
        "Middleware: aborts the request after the given duration.",
        timeout::signature,
        timeout::analyze_api_timeout,
    ),
];
