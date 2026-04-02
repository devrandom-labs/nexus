#[test]
fn macro_compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/macro_compile_fail/*.rs");
}
