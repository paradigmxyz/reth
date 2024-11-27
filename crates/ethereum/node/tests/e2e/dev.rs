use std::sync::Arc;

use alloy_eips::eip2718::Encodable2718;
use alloy_genesis::Genesis;
use alloy_primitives::{b256, hex};
use futures::StreamExt;
use reth::{args::DevArgs, rpc::api::eth::helpers::EthTransactions};
use reth_chainspec::ChainSpec;
use reth_node_api::{FullNodeComponents, FullNodePrimitives, NodeTypes};
use reth_node_builder::{
    rpc::RethRpcAddOns, EngineNodeLauncher, FullNode, NodeBuilder, NodeConfig, NodeHandle,
};
use reth_node_ethereum::{node::EthereumAddOns, EthereumNode};
use reth_provider::{providers::BlockchainProvider2, CanonStateSubscriptions};
use reth_tasks::TaskManager;

#[tokio::test]
async fn can_run_dev_node() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let node_config = NodeConfig::test()
        .with_chain(custom_chain())
        .with_dev(DevArgs { dev: true, ..Default::default() });
    let NodeHandle { node, .. } = NodeBuilder::new(node_config.clone())
        .testing_node(exec.clone())
        .with_types_and_provider::<EthereumNode, BlockchainProvider2<_>>()
        .with_components(EthereumNode::components())
        .with_add_ons(EthereumAddOns::default())
        .launch_with_fn(|builder| {
            let launcher = EngineNodeLauncher::new(
                builder.task_executor().clone(),
                builder.config().datadir(),
                Default::default(),
            );
            builder.launch_with(launcher)
        })
        .await?;

    assert_chain_advances(node).await;

    Ok(())
}

async fn assert_chain_advances<N, AddOns>(node: FullNode<N, AddOns>)
where
    N: FullNodeComponents<Provider: CanonStateSubscriptions>,
    AddOns: RethRpcAddOns<N, EthApi: EthTransactions>,
    N::Types: NodeTypes<Primitives: FullNodePrimitives>,
{
    let mut notifications = node.provider.canonical_state_stream();

    // submit tx through rpc
    let raw_tx = hex!("02f876820a28808477359400847735940082520894ab0840c0e43688012c1adb0f5e3fc665188f83d28a029d394a5d630544000080c080a0a044076b7e67b5deecc63f61a8d7913fab86ca365b344b5759d1fe3563b4c39ea019eab979dd000da04dfc72bb0377c092d30fd9e1cab5ae487de49586cc8b0090");

    let eth_api = node.rpc_registry.eth_api();

    let hash = eth_api.send_raw_transaction(raw_tx.into()).await.unwrap();

    let expected = b256!("b1c6512f4fc202c04355fbda66755e0e344b152e633010e8fd75ecec09b63398");

    assert_eq!(hash, expected);
    println!("submitted transaction: {hash}");

    let head = notifications.next().await.unwrap();

    let tx = &head.tip().transactions()[0];
    assert_eq!(tx.trie_hash(), hash);
    println!("mined transaction: {hash}");
}

fn custom_chain() -> Arc<ChainSpec> {
    let custom_genesis = r#"
{

    "nonce": "0x42",
    "timestamp": "0x0",
    "extraData": "0x5343",
    "gasLimit": "0x13880",
    "difficulty": "0x400000000",
    "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "coinbase": "0x0000000000000000000000000000000000000000",
    "alloc": {
        "0x6Be02d1d3665660d22FF9624b7BE0551ee1Ac91b": {
            "balance": "0x4a47e3c12448f4ad000000"
        }
    },
    "number": "0x0",
    "gasUsed": "0x0",
    "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "config": {
        "ethash": {},
        "chainId": 2600,
        "homesteadBlock": 0,
        "eip150Block": 0,
        "eip155Block": 0,
        "eip158Block": 0,
        "byzantiumBlock": 0,
        "constantinopleBlock": 0,
        "petersburgBlock": 0,
        "istanbulBlock": 0,
        "berlinBlock": 0,
        "londonBlock": 0,
        "terminalTotalDifficulty": 0,
        "terminalTotalDifficultyPassed": true,
        "shanghaiTime": 0
    }
}
"#;
    let genesis: Genesis = serde_json::from_str(custom_genesis).unwrap();
    Arc::new(genesis.into())
}
