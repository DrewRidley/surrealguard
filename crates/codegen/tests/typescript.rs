// // surrealguard/crates/codegen/tests/typescript.rs
// use surrealguard_macros::kind;

// #[test]
// fn test_basic_types() {
//     assert_eq!(TypeScriptGenerator::generate(&kind!("string")), "string");
//     assert_eq!(TypeScriptGenerator::generate(&kind!("number")), "number");
//     assert_eq!(TypeScriptGenerator::generate(&kind!("bool")), "boolean");
//     assert_eq!(TypeScriptGenerator::generate(&kind!("datetime")), "Date");
//     assert_eq!(
//         TypeScriptGenerator::generate(&kind!("duration")),
//         "Duration"
//     );
// }

// #[test]
// fn test_record_types() {
//     assert_eq!(
//         TypeScriptGenerator::generate(&kind!("record<user>")),
//         "(RecordId<\"user\"> & { id: string })"
//     );
// }

// #[test]
// fn test_geometry_types() {
//     assert_eq!(
//         TypeScriptGenerator::generate(&kind!("geometry<point>")),
//         "Point"
//     );
//     assert_eq!(
//         TypeScriptGenerator::generate(&kind!("geometry<polygon>")),
//         "Polygon"
//     );
// }

// #[test]
// fn test_complex_object() {
//     let kind = kind!(
//         r#"{
//         user: record<user>,
//         location: geometry<point>,
//         created: duration,
//         posts: array<record<post>>
//     }"#
//     );

//     let expected = r#"{
//   created: Duration,
//   location: Point,
//   posts: Array<(RecordId<"post"> & { id: string })>,
//   user: (RecordId<"user"> & { id: string })
// }"#;

//     assert_eq!(TypeScriptGenerator::generate(&kind), expected);
// }
