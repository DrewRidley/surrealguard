//! Flow-control statement analyzers: scoped environments and statement
//! values (a block evaluates to its last statement; IF merges its branches).

pub mod block;
pub mod break_stmt;
pub mod continue_stmt;
pub mod for_loop;
pub mod if_else;
pub mod let_stmt;
pub mod return_stmt;
pub mod throw;

#[cfg(test)]
pub(crate) mod tests {
    pub(crate) fn first_of_kind<'tree>(
        node: tree_sitter::Node<'tree>,
        kind: &str,
    ) -> tree_sitter::Node<'tree> {
        fn walk<'tree>(
            node: tree_sitter::Node<'tree>,
            kind: &str,
        ) -> Option<tree_sitter::Node<'tree>> {
            if node.kind() == kind {
                return Some(node);
            }
            let mut cursor = node.walk();
            let found = node
                .children(&mut cursor)
                .find_map(|child| walk(child, kind));
            found
        }
        walk(node, kind).unwrap_or_else(|| panic!("no {kind} node in tree"))
    }
}
