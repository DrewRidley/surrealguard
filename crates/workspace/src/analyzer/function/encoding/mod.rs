//! `encoding` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod base64_decode;
pub mod base64_encode;
pub mod cbor_decode;
pub mod cbor_encode;
pub mod json_decode;
pub mod json_encode;

/// Every `encoding::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "encoding::base64::decode",
        "Decodes a base64 string into bytes.",
        base64_decode::signature,
        base64_decode::analyze_encoding_base64_decode,
    ),
    BuiltinEntry::new(
        "encoding::base64::encode",
        "Encodes bytes as a base64 string.",
        base64_encode::signature,
        base64_encode::analyze_encoding_base64_encode,
    ),
    BuiltinEntry::new(
        "encoding::cbor::decode",
        "Decodes CBOR bytes into a value.",
        cbor_decode::signature,
        cbor_decode::analyze_encoding_cbor_decode,
    ),
    BuiltinEntry::new(
        "encoding::cbor::encode",
        "Encodes a value as CBOR bytes.",
        cbor_encode::signature,
        cbor_encode::analyze_encoding_cbor_encode,
    ),
    BuiltinEntry::new(
        "encoding::json::decode",
        "Parses a JSON string into a value.",
        json_decode::signature,
        json_decode::analyze_encoding_json_decode,
    ),
    BuiltinEntry::new(
        "encoding::json::encode",
        "Serializes a value as a JSON string.",
        json_encode::signature,
        json_encode::analyze_encoding_json_encode,
    ),
];
