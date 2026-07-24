use eyre::Result;
use reth_rpc_compat_tests::{config::RunConfig, fixture::Fixture, run_embedded};
use std::path::PathBuf;

#[tokio::test(flavor = "multi_thread")]
async fn local_rpc_compat_fixture() -> Result<()> {
    reth_tracing::init_test_tracing();
    let tests = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/rpc-compat");
    let fixture = Fixture {
        root: tests.parent().unwrap().to_path_buf(),
        tests,
        revision: "repo-local".to_string(),
    };
    let run = reth_rpc_compat_tests::config::ResolvedRunConfig {
        include: RunConfig::default().include,
        exclude: Vec::new(),
        skip: Vec::new(),
        ignore: Vec::new(),
        expected_failures: Vec::new(),
        responses: Default::default(),
        selections: Default::default(),
        timeout_secs: 5,
        fail_fast: false,
        report_json: None,
        report_junit: None,
    };
    run_embedded(fixture, run).await
}
