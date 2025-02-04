
use surrealdb::sql::parse;
use surrealguard_macros::kind;

fn main() {
    let stmt = "SELECT VALUE email FROM user;";
    let parsed = parse(stmt);

    println!("Parsed value: \n{:#?}", parsed);
}
