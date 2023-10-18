//! auto-mine consensus integration test
use std::{sync::Arc, path::{PathBuf, Path}, io::{Write, BufWriter}};

use jsonrpsee::{http_client::HttpClientBuilder, rpc_params, core::client::ClientT};
use reth::{node::NodeCommand, runner::CliRunner, cli::{components::RethNodeComponents, ext::{RethNodeCommandConfig, NoArgs, NoArgsCliExt}}, tasks::TaskSpawner};
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::{Chain, ChainSpec, ChainSpecBuilder, Genesis, hex, revm_primitives::HashMap, U256, GenesisAccount, Address, stage::StageId, Head};
use reth_db:: DatabaseEnv;
use reth_provider::{ProviderFactory, CanonStateSubscriptions, CanonStateNotification};
use clap::Parser;
use reth_transaction_pool::TransactionPool;
use tempfile::NamedTempFile;
use tokio::sync::{mpsc, oneshot};
use tracing::debug;

// #[derive(Debug)]
// struct TestExt;
// impl RethCliExt for TestExt {
//     type Node = AutoMineConfig;
// }

#[derive(Debug)]
struct AutoMineConfig(mpsc::Sender<CanonStateNotification>);

impl RethNodeCommandConfig for AutoMineConfig {
    fn on_components_initialized<Reth: RethNodeComponents>(
        &mut self,
        _components: &Reth,
    ) -> eyre::Result<()> {
        tokio::spawn(async move {
            println!("All components initialized");
        });
        Ok(())
    }
    fn on_node_started<Reth: RethNodeComponents>(
        &mut self,
        components: &Reth,
    ) -> eyre::Result<()> {
        debug!("inside on-node-started");
        let pool = components.pool();
        let size = pool.pool_size();
        println!("size {size:?}");
        let mut canon_events = components.events().subscribe_to_canonical_state();

        let sender = self.0.clone();

        components.task_executor().spawn_critical_blocking("rpc request", Box::pin(async move {
            debug!("spawning update listener...");
            let raw_tx = "0xf86f0284430e234082520894ab0840c0e43688012c1adb0f5e3fc665188f83d28a029d394a5d630544000080821473a053ebe89f637e1d6ee353b3e49c1e5fba5bbff889adfcd11f78e31caa998a2feaa067f543fec289232bda1648117bdf008ebb640c3140d18e3dbef4c166199ff743";
            let client = HttpClientBuilder::default().build("http://127.0.0.1:8545").expect("");
            let response: String = client.request("eth_sendRawTransaction", rpc_params![raw_tx]).await.expect("");
            let expected = "0x1dbd858accfe5a979e2cd9d0d21660ef7c6e622810b5d7b7f71215301b200906";
            debug!("response: {response:?}");
            assert_eq!(&response, expected);
            debug!("waiting for canon state update...");
            let update = canon_events.recv().await.expect("canon state updated");
            let expected = CanonStateNotification::Commit { new: Default::default() };
            assert_eq!(update, expected);
        }));
        Ok(())
    }

    fn on_rpc_server_started<Conf, Reth>(
        &mut self,
        config: &Conf,
        components: &Reth,
        rpc_components: reth::cli::components::RethRpcComponents<'_, Reth>,
        handles: reth::cli::components::RethRpcServerHandles,
    ) -> eyre::Result<()>
    where
        Conf: reth::cli::config::RethRpcConfig,
        Reth: RethNodeComponents,
    {
        let _ = config;
        let _ = components;
        let _ = rpc_components;
        let _ = handles;
        Ok(())
    }
}

// #[tokio::test]
#[test]
pub fn test_auto_mine() {
    reth_tracing::init_test_tracing();
    // let chain = build_chain_spec();
    let temp_path = tempfile::TempDir::new()
        .expect("tempdir is okay")
        .into_path();

    let datadir = temp_path.to_str().expect("temp path is okay");
    debug!("datadir: {datadir:?}");

    let (tx, mut rx) = mpsc::channel(1);
    let no_args = NoArgs::with(AutoMineConfig(tx));
    // let chain_file = write_custom_chain_json(&chain_path);
    // let chain_path = chain_file.to_str().expect("chain spec path okay");

    // let mut file = tempfile::tempfile_in("./").unwrap();
    // let chain_path = "./chain-spec";
    let mut file = tempfile::NamedTempFile::new_in("./").unwrap();
    
    let chain_spec = custom_chain(&file);
    write!(file, "{chain_spec:?}").unwrap();
    let chain_path = file.path().to_str().unwrap();

    println!("{:?}", std::fs::read_to_string(&chain_path).unwrap());
    
    let command = NodeCommand::<NoArgsCliExt<AutoMineConfig>>::parse_from([
    // let mut command = NodeCommand::<()>::parse_from([
        "reth",
        "--dev",
        "--datadir",
        datadir,
        "--debug.max-block",
        "1",
        "--chain",
        chain_path,
    ])
    .with_ext::<NoArgsCliExt<AutoMineConfig>>(no_args);
    // add custom chain
    // command.chain = chain;

    let runner = CliRunner::default();
    runner.run_command_until_exit(|ctx| command.execute(ctx)).unwrap();
    // let res = rx.try_recv().unwrap();

}

/// Build custom chain spec.
fn build_chain_spec() -> Arc<ChainSpec> {
    let chain = Chain::Id(2600);
    let genesis = build_genesis();
    let mut chain_spec = ChainSpecBuilder::default()
        .chain(chain)
        .cancun_activated()
        .genesis(genesis)
        .build();

    let genesis_hash = chain_spec.genesis_hash();
    chain_spec.genesis_hash = Some(genesis_hash);

    Arc::new(chain_spec)
}

/// Build genesis for chain spec.
fn build_genesis() -> Genesis {
    // bob well-known private key: 0x99b3c12287537e38c90a9219d4cb074a89a16e9cdb20bf85728ebd97c343e342
    let bob_address = hex!("6Be02d1d3665660d22FF9624b7BE0551ee1Ac91b").into();
    let bob_account = GenesisAccount::default().with_balance(U256::from(10_000));

    let accounts: HashMap<Address, GenesisAccount> = HashMap::from([
        (bob_address, bob_account),
    ]);

    Genesis::default().extend_accounts(accounts)
}

fn custom_chain(file: &NamedTempFile) {
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
    // let genesis: Genesis = serde_json::from_str(custom_genesis).unwrap();
    // genesis
    // custom_genesis.to_string()

    // let custom_genesis: Genesis = serde_json::from_str(custom_genesis).unwrap();
    let genesis: Genesis = serde_json::from_str(custom_genesis).unwrap();
    println!("\n\ngenesis\n\n{genesis:?}\n\n\n");

    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, &genesis).unwrap();
    writer.flush().unwrap();
}

#[tokio::test]
async fn submit_tx() {
    let raw_tx = "0xf86f0284430e234082520894ab0840c0e43688012c1adb0f5e3fc665188f83d28a029d394a5d630544000080821473a053ebe89f637e1d6ee353b3e49c1e5fba5bbff889adfcd11f78e31caa998a2feaa067f543fec289232bda1648117bdf008ebb640c3140d18e3dbef4c166199ff743";
    let client = HttpClientBuilder::default().build("http://127.0.0.1:8545").expect("");
    let response: String = client.request("eth_sendRawTransaction", rpc_params![raw_tx]).await.expect("");
    debug!("response: {response:?}");
}