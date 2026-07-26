// `$min` is compared against `age: int`, so a string cannot be bound to it.
fn main() {
    let _ = surrealguard_rs::query!("SELECT name FROM user WHERE age > $min;", min = "18");
}
