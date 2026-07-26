// `RETURN 1 + 1` yields an `i64`, which is not a row set — `fetch_all` must not
// resolve. `.fetch()` is the verb for it.
use surrealdb::engine::remote::ws::Client;
use surrealdb::Surreal;

async fn run(db: &Surreal<Client>) {
    let _ = surrealguard_rs::query!("RETURN 1 + 1;").fetch_all(db).await;
}

fn main() {}
