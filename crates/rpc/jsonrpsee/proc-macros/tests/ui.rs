//! UI test set uses [`trybuild`](https://docs.rs/trybuild/1.0.42/trybuild/) to
//! check whether expected valid examples of code compile correctly, and for incorrect ones
//! errors are helpful and valid (e.g. have correct spans).
//!
//! Use with `TRYBUILD=overwrite` after updating codebase (see `trybuild` docs for more details on that)
//! to automatically regenerate `stderr` files, but don't forget to check that new files make sense.

#[test]
fn ui_pass() {
	let t = trybuild::TestCases::new();
	t.pass("tests/ui/correct/*.rs");
}

#[test]
fn ui_fail() {
	let t = trybuild::TestCases::new();
	t.compile_fail("tests/ui/incorrect/**/*.rs");
}
