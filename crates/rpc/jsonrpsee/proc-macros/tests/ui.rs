//! UI tests that check the valid examples in `tests/ui/correct` compile.

#[test]
fn ui_pass() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/correct/*.rs");
}
