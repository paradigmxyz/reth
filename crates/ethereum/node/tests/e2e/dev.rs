use alloy_genesis::Genesis;
use alloy_primitives::{b256, hex, Address};
use futures::StreamExt;
use reth_chainspec::ChainSpec;
use reth_e2e_test_utils::E2ETestSetupExt;
use reth_node_api::{BlockBody, FullNodeComponents};
use reth_node_builder::{rpc::RethRpcAddOns, FullNode};
use reth_node_ethereum::EthereumNode;
use reth_primitives_traits::transaction::TxHashRef;
use reth_provider::{BlockIdReader, BlockNumReader, CanonStateSubscriptions};
use reth_rpc_eth_api::{helpers::EthTransactions, EthApiServer};
use std::{num::NonZeroUsize, sync::Arc, time::Duration};

#[tokio::test]
async fn can_run_dev_node() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (node, _) = EthereumNode::test_setup(1, custom_chain())
        .with_dev_mining(None)
        .with_node_config_modifier(|mut config| {
            config.dev.finality_depth = NonZeroUsize::new(1).unwrap();
            config
        })
        .build_single()
        .await?;
    let node = &node.inner;

    let canon_state = node.provider.canonical_in_memory_state();
    let mut safe_block = canon_state.subscribe_safe_block();
    let mut finalized_block = canon_state.subscribe_finalized_block();

    assert_chain_advances(node).await;

    let chain_info = node.provider.chain_info()?;
    // Startup can leave an unread genesis notification, and the canonical head notification
    // precedes the safe/finalized updates. Wait for the mined block itself on both channels.
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::try_join!(
            safe_block.wait_for(|header| {
                header.as_ref().is_some_and(|header| header.num_hash() == chain_info.into())
            }),
            finalized_block.wait_for(|header| {
                header.as_ref().is_some_and(|header| header.num_hash() == chain_info.into())
            }),
        )
        .map(|_| ())
    })
    .await??;

    assert_eq!(node.provider.safe_block_num_hash()?, Some(chain_info.into()));
    assert_eq!(node.provider.finalized_block_num_hash()?, Some(chain_info.into()));

    Ok(())
}

#[tokio::test]
async fn can_run_dev_node_custom_attributes() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let fee_recipient = Address::random();
    let (node, _) = EthereumNode::test_setup(1, custom_chain())
        .with_dev_mining(None)
        .map_dev_payload_attributes(move |mut attributes| {
            attributes.suggested_fee_recipient = fee_recipient;
            attributes
        })
        .build_single()
        .await?;
    let node = &node.inner;

    assert_chain_advances(node).await;

    assert!(
        node.rpc_registry.eth_api().balance(fee_recipient, Default::default()).await.unwrap() > 0
    );

    assert!(
        node.rpc_registry
            .eth_api()
            .block_by_number(Default::default(), false)
            .await
            .unwrap()
            .unwrap()
            .header
            .beneficiary ==
            fee_recipient
    );

    Ok(())
}

#[tokio::test]
async fn can_run_dev_node_with_block_time() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (node, _) = EthereumNode::test_setup(1, custom_chain())
        .with_dev_mining(Some(Duration::from_millis(100)))
        .build_single()
        .await?;

    // The local miner builds a block on every interval, even without pending transactions.
    tokio::time::timeout(Duration::from_secs(10), async {
        while node.inner.provider.best_block_number()? < 2 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        eyre::Ok(())
    })
    .await??;

    Ok(())
}

async fn assert_chain_advances<N, AddOns>(node: &FullNode<N, AddOns>)
where
    N: FullNodeComponents<Provider: CanonStateSubscriptions>,
    AddOns: RethRpcAddOns<N, EthApi: EthTransactions>,
{
    let mut notifications = node.provider.canonical_state_stream();

    // submit tx through rpc
    let raw_tx = hex!(
        "02f876820a28808477359400847735940082520894ab0840c0e43688012c1adb0f5e3fc665188f83d28a029d394a5d630544000080c080a0a044076b7e67b5deecc63f61a8d7913fab86ca365b344b5759d1fe3563b4c39ea019eab979dd000da04dfc72bb0377c092d30fd9e1cab5ae487de49586cc8b0090"
    );

    let eth_api = node.rpc_registry.eth_api();

    let hash = eth_api.send_raw_transaction(raw_tx.into()).await.unwrap();

    let expected = b256!("0xb1c6512f4fc202c04355fbda66755e0e344b152e633010e8fd75ecec09b63398");

    assert_eq!(hash, expected);
    println!("submitted transaction: {hash}");

    let head = notifications.next().await.unwrap();

    let tx = &head.tip().body().transactions()[0];
    assert_eq!(*tx.tx_hash(), hash);
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
