//! Compile-fail tests: `#[tool]` reports misuse at the exact span.
//!
//! The expected compiler output lives in `tests/ui/*.stderr`. After an
//! intentional change, regenerate it with
//! `TRYBUILD=overwrite cargo test -p mcpsdkrust --test ui` and review the diff.

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
