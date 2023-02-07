//! Standalone http tests

use crate::utils::test_rpc_builder;

#[tokio::test(flavor = "multi_thread")]
async fn test_launch_http() {
    let _builder = test_rpc_builder();
}
