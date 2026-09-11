// The query reads no `$limit`, so supplying one is a typo, not a binding.
fn main() {
    let _ = surrealql_analyzer_rs::query!("SELECT name FROM user;", limit = 10);
}
