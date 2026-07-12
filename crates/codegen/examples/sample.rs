use surrealdb_types::{Kind, KindLiteral};
use surrealguard_codegen::{render_registry, ts_type, QueryEntry};
use surrealguard_workspace::analysis::ParamInference;

fn main() {
    let mut row = std::collections::BTreeMap::new();
    row.insert("name".to_string(), Kind::String);
    row.insert("age".to_string(), Kind::Int);
    let result = Kind::Array(Box::new(Kind::Literal(KindLiteral::Object(row))), None);

    let entries = vec![
        QueryEntry {
            parts: vec![
                "SELECT name, age FROM person WHERE age > ".into(),
                "".into(),
            ],
            result_type: ts_type(&result),
            params: vec![ParamInference {
                name: "__host0".into(),
                kind: Some(Kind::Int),
                domain: None,
                required: true,
                spans: Vec::new(),
            }],
        },
        QueryEntry {
            parts: vec!["SELECT name FROM person WHERE team = $team".into()],
            result_type: "Array<{ name: string }>".into(),
            params: vec![ParamInference {
                name: "team".into(),
                kind: Some(Kind::String),
                domain: None,
                required: true,
                spans: Vec::new(),
            }],
        },
    ];
    print!("{}", render_registry(&entries));
}
