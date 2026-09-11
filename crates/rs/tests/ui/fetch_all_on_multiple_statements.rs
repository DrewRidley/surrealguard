// Two responding statements decode into a tuple, which is not a row set.
use surrealdb::engine::remote::ws::Client;
use surrealdb::Surreal;

async fn run(db: &Surreal<Client>) {
    let _ = surrealql_analyzer_rs::query!("SELECT name FROM user; SELECT title FROM post;")
        .fetch_all(db)
        .await;
}

fn main() {}
