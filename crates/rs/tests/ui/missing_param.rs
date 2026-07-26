// `$min` is required by the query and not supplied.
fn main() {
    let _ = surrealguard_rs::query!("SELECT name FROM user WHERE age > $min;");
}
