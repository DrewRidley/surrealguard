//! Rust bindings for the SurrealQL tree-sitter grammar.
//!
//! The grammar sources in this crate are vendored verbatim from the merged
//! official grammar [`surrealdb/surrealql-tree-sitter`](https://github.com/surrealdb/surrealql-tree-sitter)
//! (commit `f40a0ede750081b8d49feb40516c28cc49f1cce4`). This thin binding shim
//! (`bindings/rust/*`) is maintained by SurrealGuard. The crate is published as
//! `surrealguard-tree-sitter-surrealql` but stays importable as
//! `tree_sitter_surrealql`.

use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_surrealql() -> *const ();
}

/// Returns the tree-sitter [`LanguageFn`] for SurrealQL.
///
/// # Example
///
/// ```
/// let language = tree_sitter_surrealql::LANGUAGE;
/// let mut parser = tree_sitter::Parser::new();
/// parser.set_language(&language.into()).expect("Failed to set language");
/// ```
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_surrealql) };

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_load_grammar() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&LANGUAGE.into())
            .expect("Failed to load SurrealQL grammar");
    }

    #[test]
    fn test_parse_simple_query() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&LANGUAGE.into())
            .expect("Failed to load grammar");

        let tree = parser
            .parse("SELECT * FROM person;", None)
            .expect("Failed to parse");
        let root = tree.root_node();

        assert_eq!(root.kind(), "SurrealQL");
        assert!(!root.has_error());
    }
}
