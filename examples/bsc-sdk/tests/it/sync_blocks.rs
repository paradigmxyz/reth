use example_bsc_sdk::{chainspec::bsc::bsc_mainnet, node::BscNode};
use reth::{
    args::RpcServerArgs,
    builder::{NodeBuilder, NodeConfig, NodeHandle},
    tasks::TaskManager,
};

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn can_sync_blocks() -> eyre::Result<()> {
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let node_config = NodeConfig::new(bsc_mainnet())
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let NodeHandle { node: _, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
        .testing_node(exec.clone())
        .node(BscNode::default())
        .launch()
        .await?;

    Ok(())
}
