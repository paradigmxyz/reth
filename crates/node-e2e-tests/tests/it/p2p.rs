use node_e2e_tests::{test_node::TestNode, test_suite::TestSuite};
use reth::tasks::TaskManager;

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let tasks = TaskManager::current();
    let test_suite = TestSuite::new();

    let mut first_node = TestNode::new(test_suite.chain_spec(), tasks.executor()).await?;
    let mut second_node = TestNode::new(test_suite.chain_spec(), tasks.executor()).await?;

    first_node.add_peer(second_node.node_record).await;
    second_node.add_peer(first_node.node_record).await;

    first_node.assert_session_established().await;
    second_node.assert_session_established().await;

    Ok(())
}
