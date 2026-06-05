#[test]
fn api_type_compile_failures_are_clear() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/api_type_tuple_variant.rs");
    tests.compile_fail("tests/ui/api_type_unsupported_field.rs");
    tests.compile_fail("tests/ui/api_type_unsupported_rename_all.rs");
    tests.compile_fail("tests/ui/api_error_missing_status.rs");
    tests.compile_fail("tests/ui/api_error_invalid_status.rs");
    tests.compile_fail("tests/ui/api_endpoint_missing_path.rs");
    tests.compile_fail("tests/ui/api_endpoint_non_async.rs");
    tests.compile_fail("tests/ui/api_endpoint_invalid_return.rs");
    tests.compile_fail("tests/ui/api_endpoint_invalid_error.rs");
    tests.compile_fail("tests/ui/api_module_missing_endpoint.rs");
    tests.compile_fail("tests/ui/api_type_unsupported_external_generic.rs");
}
