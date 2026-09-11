//! Dev tool: dump CSTs to design/verify lowering against real grammar shapes.
//!
//! Usage: `cargo run -p surrealql-analyzer-syntax --example dump_cst [-- "QUERY"]`

fn dump(node: tree_sitter::Node, src: &str, depth: usize) {
    if node.is_named() {
        let text: String = src[node.byte_range()].chars().take(40).collect();
        println!("{}{} {:?}", "  ".repeat(depth), node.kind(), text);
    }
    let mut c = node.walk();
    for ch in node.children(&mut c) {
        dump(ch, src, depth + usize::from(node.is_named()));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let defaults = [
        "SELECT * FROM ONLY person:one;",
        "SELECT VALUE age FROM person;",
        "SELECT name AS display, age FROM person, company;",
        "SELECT * OMIT password FROM person FETCH profile SPLIT tags;",
        "SELECT * FROM person ORDER BY name DESC, age LIMIT 5 START 10;",
        "SELECT * FROM person GROUP BY city;",
        "SELECT * FROM person GROUP ALL;",
        "SELECT * FROM $tbl;",
        "SELECT * FROM (SELECT * FROM person);",
        "SELECT * FROM person TIMEOUT 5s PARALLEL EXPLAIN;",
        "SELECT * FROM person RETURN NONE;",
    ];
    let queries: Vec<&str> = if args.is_empty() {
        defaults.to_vec()
    } else {
        args.iter().map(String::as_str).collect()
    };

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_surrealql::LANGUAGE.into())
        .unwrap();
    for q in queries {
        println!("==== {q}");
        let tree = parser.parse(q, None).unwrap();
        dump(tree.root_node(), q, 0);
    }
}
