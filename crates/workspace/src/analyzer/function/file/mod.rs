//! `file` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

pub mod bucket;
pub mod copy;
pub mod copy_if_not_exists;
pub mod delete;
pub mod exists;
pub mod get;
pub mod head;
pub mod key;
pub mod list;
pub mod put;
pub mod put_if_not_exists;
pub mod rename;
pub mod rename_if_not_exists;

/// Every `file::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "file::bucket",
        "The bucket name of a file pointer.",
        bucket::signature,
        bucket::analyze_file_bucket,
    ),
    BuiltinEntry::new(
        "file::copy",
        "Copies a file to a new key in its bucket, overwriting any existing file.",
        copy::signature,
        copy::analyze_file_copy,
    ),
    BuiltinEntry::new(
        "file::copy_if_not_exists",
        "Copies a file to a new key in its bucket unless the target exists.",
        copy_if_not_exists::signature,
        copy_if_not_exists::analyze_file_copy_if_not_exists,
    ),
    BuiltinEntry::new(
        "file::delete",
        "Deletes a file from its bucket.",
        delete::signature,
        delete::analyze_file_delete,
    ),
    BuiltinEntry::new(
        "file::exists",
        "Whether a file exists in its bucket.",
        exists::signature,
        exists::analyze_file_exists,
    ),
    BuiltinEntry::new(
        "file::get",
        "The raw contents of a file.",
        get::signature,
        get::analyze_file_get,
    ),
    BuiltinEntry::new(
        "file::head",
        "The metadata of a file, or NONE.",
        head::signature,
        head::analyze_file_head,
    ),
    BuiltinEntry::new(
        "file::key",
        "The key of a file pointer within its bucket.",
        key::signature,
        key::analyze_file_key,
    ),
    BuiltinEntry::new(
        "file::list",
        "Lists the files in a bucket, with optional filtering options.",
        list::signature,
        list::analyze_file_list,
    ),
    BuiltinEntry::new(
        "file::put",
        "Writes contents to a file, overwriting any existing file.",
        put::signature,
        put::analyze_file_put,
    ),
    BuiltinEntry::new(
        "file::put_if_not_exists",
        "Writes contents to a file unless it already exists.",
        put_if_not_exists::signature,
        put_if_not_exists::analyze_file_put_if_not_exists,
    ),
    BuiltinEntry::new(
        "file::rename",
        "Renames a file within its bucket, overwriting any existing file.",
        rename::signature,
        rename::analyze_file_rename,
    ),
    BuiltinEntry::new(
        "file::rename_if_not_exists",
        "Renames a file within its bucket unless the target exists.",
        rename_if_not_exists::signature,
        rename_if_not_exists::analyze_file_rename_if_not_exists,
    ),
];
