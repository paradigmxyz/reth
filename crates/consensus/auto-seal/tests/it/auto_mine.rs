//! auto-mine consensus integration test
use std::sync::Arc;

use jsonrpsee::{http_client::HttpClientBuilder, rpc_params, core::client::ClientT};
use reth::{node::NodeCommand, runner::CliRunner, cli::{components::RethNodeComponents, ext::{RethNodeCommandConfig, NoArgs, NoArgsCliExt}}};
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::{Chain, ChainSpec, ChainSpecBuilder, Genesis, hex, revm_primitives::HashMap, U256, GenesisAccount, Address, stage::StageId, Head};
use reth_db:: DatabaseEnv;
use reth_provider::{ProviderFactory, CanonStateSubscriptions, CanonStateNotification};
use clap::Parser;
use reth_transaction_pool::TransactionPool;
use tokio::sync::{mpsc, oneshot};

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
        let pool = components.pool();
        let size = pool.pool_size();
        println!("size {size:?}");
        let mut canon_events = components.events().subscribe_to_canonical_state();
        let sender = self.0.clone();
        tokio::spawn(async move {
            println!("spawning update listener...");
            let update = canon_events.recv().await.expect("canon state updated");
            sender.send(update).await.unwrap();
        });
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
        tokio::task::spawn_blocking(|| async move {
            let raw_tx = "0xf86c808252088252089481b1b34d8f1d323b1088b5cbf32989dc143170a989120d4da7b0bd14000080821473a06fafb8a230168cb6e6b57579f48d65d8d7a0acfae4c7d64f9af9eb78a3cf938ea0090b61bdc4c64816e0148dde7d645cd2ea6e435a8c68018a472931d83ce1f29e";
            let client = HttpClientBuilder::default().build("http://127.0.0.1:8545").expect("");
            let response: String = client.request("send_raw_transaction", rpc_params![raw_tx]).await.expect("");
            println!("response: {response:?}");
        });
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
    let chain = build_chain_spec();
    let temp_path = tempfile::TempDir::new()
        .expect("tempdir is okay")
        .into_path();
    let datadir = temp_path.to_str().expect("temp path is okay");

    let (tx, mut rx) = mpsc::channel(1);
    let no_args = NoArgs::with(AutoMineConfig(tx));
    let mut command = NodeCommand::<NoArgsCliExt<AutoMineConfig>>::parse_from([
        "reth",
        "--dev",
        "--datadir",
        datadir,
        "--debug.max-block",
        "1",
        // "--debug.terminate",
    ])
    .with_ext::<NoArgsCliExt<AutoMineConfig>>(no_args);
    // add custom chain
    command.chain = chain;

    let runner = CliRunner::default();
    let res = runner.run_command_until_exit(|ctx| command.execute(ctx));
    let (tx, done) = oneshot::channel();
    tokio::spawn(async move {
        let res = rx.recv().await.unwrap();
        println!("res: {res:?}");
        let _ = tx.send(res);
    });
    let _ = done.blocking_recv();

    println!("res {res:?}");
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
