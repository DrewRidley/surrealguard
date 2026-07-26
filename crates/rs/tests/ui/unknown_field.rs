// A field the schema does not have is an analyzer error at compile time.
fn main() {
    let _ = surrealguard_rs::query!("SELECT name, ssn FROM user;");
}
