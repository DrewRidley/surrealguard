// Supplying the same parameter twice is a mistake, not a last-one-wins.
fn main() {
    let _ = surrealguard_rs::query!(
        "SELECT name FROM user WHERE age > $min;",
        min = 18,
        min = 21,
    );
}
