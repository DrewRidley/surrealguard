// `$min` is compared against `age: int`, so a string cannot be bound to it.
fn main() {
    let _ = surrealql_analyzer_rs::query!("SELECT name FROM user WHERE age > $min;", min = "18");
}
