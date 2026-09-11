// `$min` is required by the query and not supplied.
fn main() {
    let _ = surrealql_analyzer_rs::query!("SELECT name FROM user WHERE age > $min;");
}
