//! auto-mine consensus integration test

use clap::Parser;
use jsonrpsee::{core::client::ClientT, http_client::HttpClientBuilder, rpc_params};

use reth_primitives::{hex, revm_primitives::FixedBytes, ChainSpec, Genesis};
use reth_provider::CanonStateSubscriptions;
use reth_transaction_pool::TransactionPool;
use std::{sync::Arc, time::Duration};
use tokio::time::timeout;

use reth::{
    cli::{
        components::RethNodeComponents,
        ext::{NoArgs, NoArgsCliExt, RethNodeCommandConfig},
    },
    commands::node::NodeCommand,
    runner::CliRunner,
    tasks::TaskSpawner,
};

#[derive(Debug)]
struct AutoMineConfig;

impl RethNodeCommandConfig for AutoMineConfig {
    fn on_node_started<Reth: RethNodeComponents>(&mut self, components: &Reth) -> eyre::Result<()> {
        let pool = components.pool();
        let mut canon_events = components.events().subscribe_to_canonical_state();

        components.task_executor().spawn_critical_blocking("rpc request", Box::pin(async move {
            // submit tx through rpc
            let raw_tx = "0x02f876820a28808477359400847735940082520894ab0840c0e43688012c1adb0f5e3fc665188f83d28a029d394a5d630544000080c080a0a044076b7e67b5deecc63f61a8d7913fab86ca365b344b5759d1fe3563b4c39ea019eab979dd000da04dfc72bb0377c092d30fd9e1cab5ae487de49586cc8b0090";
            let client = HttpClientBuilder::default().build("http://127.0.0.1:8545").expect("http client should bind to default rpc port");
            let response: String = client.request("eth_sendRawTransaction", rpc_params![raw_tx]).await.expect("client request should be valid");
            let expected = "0xb1c6512f4fc202c04355fbda66755e0e344b152e633010e8fd75ecec09b63398";

            assert_eq!(&response, expected);

            // more than enough time for the next block
            let duration = Duration::from_secs(15);

            // wait for canon event or timeout
            let update = timeout(duration, canon_events.recv())
                .await
                .expect("canon state should change before timeout")
                .expect("canon events stream is still open");
            let new_tip = update.tip();
            let expected_tx_root: FixedBytes<32> = hex!("c79b5383458e63fb20c6a49d9ec7917195a59003a2af4b28a01d7c6fbbcd7e35").into();
            assert_eq!(new_tip.transactions_root, expected_tx_root);
            assert_eq!(new_tip.number, 1);
            assert!(pool.pending_transactions().is_empty());
        }));
        Ok(())
    }
}

/// This test is disabled for the `optimism` feature flag due to an incompatible feature set.
/// L1 info transactions are not included automatically, which are required for `op-reth` to
/// process transactions.
#[test]
#[cfg_attr(feature = "optimism", ignore)]
pub(crate) fn test_auto_mine() {
    // create temp path for test
    let temp_path = tempfile::TempDir::new().expect("tempdir is okay").into_path();
    let datadir = temp_path.to_str().expect("temp path is okay");

    let no_args = NoArgs::with(AutoMineConfig);
    let chain = custom_chain();
    let mut command = NodeCommand::<NoArgsCliExt<AutoMineConfig>>::parse_from([
        "reth",
        "--dev",
        "--datadir",
        datadir,
        "--debug.max-block",
        "1",
        "--debug.terminate",
    ])
    .with_ext::<NoArgsCliExt<AutoMineConfig>>(no_args);

    // use custom chain spec
    command.chain = chain;

    let runner = CliRunner::default();
    let node_command = runner.run_command_until_exit(|ctx| command.execute(ctx));
    assert!(node_command.is_ok())
}

fn custom_chain() -> Arc<ChainSpec> {
    let custom_genesis = r#"
{
    "nonce": "0x42",
    "timestamp": "0x0",
    "extraData": "0x5343",
    "gasLimit": "0x1388",
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
