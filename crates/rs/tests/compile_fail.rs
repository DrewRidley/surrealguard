//! What must *not* compile.
//!
//! The compile-time guarantee is the product, so every rejection it promises
//! gets a case here. `trybuild` compiles each `tests/ui/*.rs` and compares the
//! compiler's output against the matching `.stderr`.

#[test]
fn rejections_are_compile_errors() {
    // The macro resolves the schema from `CARGO_MANIFEST_DIR`, which trybuild
    // repoints at its own generated crate — so name the schema explicitly.
    let schema = concat!(env!("CARGO_MANIFEST_DIR"), "/schema.surql");
    // SAFETY: single-threaded test setup, before any thread is spawned.
    unsafe { std::env::set_var("SURREALGUARD_SCHEMA", schema) };

    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}
