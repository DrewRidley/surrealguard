//! Valid queries pass compile-time checking and expand to their text.

use surrealql_analyzer_macros::surql;

#[test]
fn valid_queries_compile_and_expand_to_text() {
    let q: &str = surql!("RETURN 1 + 1;");
    assert_eq!(q, "RETURN 1 + 1;");

    // Schema-independent checks that must NOT fire on valid input.
    let _ = surql!("RETURN string::len('hello');");
    let _ = surql!("LET $x = 1; RETURN $x + 2;");
}
