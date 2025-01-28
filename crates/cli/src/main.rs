use analyzer::{context::AnalyzerContext, statements::analyze};
use surrealguard_core::*;

fn main() {
    let statement = r##"
        SELECT * FROM person;
           SELECT address.city FROM person;
           SELECT name, address FROM person;
           SELECT (( celsius * 1.8 ) + 32) AS fahrenheit FROM temperature;
           SELECT rating >= 4 as positive FROM review;
   -- Select manually generated object structure
   SELECT
	{ weekly: false, monthly: true } AS `marketing settings`
   FROM user;
           SELECT address[WHERE active = true] FROM person;
           SELECT * FROM person WHERE ->(reacted_to WHERE type='celebrate')->post;
           SELECT *, (SELECT * FROM events WHERE type = 'activity' LIMIT 5) AS history FROM user;
           SELECT address.{city, country} FROM person;
           SELECT * FROM eperson:1..1000;
           SELECT * FROM temperature:['London', NONE]..=['London', time::now()];
           SELECT * OMIT password, opt.security FROM person;
           SELECT * FROM user SPLIT emails;
           SELECT country FROM user GROUP BY country;
           SELECT *, artist.email FROM review FETCH artist;
           SELECT * FROM ONLY person:john;
           SELECT * FROM user:john VERSION d'2024-08-19T08:00:00Z';
           SELECT ->likes as likes FROM person;
           SELECT ->likes->person as likesPeople FROM person;
    "##;

    let queries = statement.split(';')
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>();

    for query in queries {
        match surrealdb::sql::parse(query) {
            Ok(stmt) => println!("Parsed query: \n{:#?}", stmt),
            Err(e) => println!("Error parsing query '{}': {:?}", query, e),
        }
    }
}
