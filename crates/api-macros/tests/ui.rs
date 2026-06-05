#[test]
fn api_type_compile_failures_are_clear() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/api_type_tuple_variant.rs");
    tests.compile_fail("tests/ui/api_type_unsupported_field.rs");
    tests.compile_fail("tests/ui/api_type_unsupported_rename_all.rs");
}
