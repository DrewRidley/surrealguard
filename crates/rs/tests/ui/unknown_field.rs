// A field the schema does not have is an analyzer error at compile time.
fn main() {
    let _ = surrealql_analyzer_rs::query!("SELECT name, ssn FROM user;");
}
