//! Demonstrates how `--engine.persistence-threshold 0` makes canonical blocks
//! visible on disk immediately for external readers, and how higher thresholds
//! delay visibility until enough blocks are produced.

#![warn(unused_crate_dependencies)]

use std::{sync::Arc, time::Duration};

use alloy_genesis::{Genesis, GenesisAccount};
use alloy_network::{
    eip2718::Encodable2718, Ethereum, EthereumWallet, NetworkWallet, TransactionBuilder,
};
use alloy_primitives::{Address, Bytes, B256, U256};
use alloy_signer_local::PrivateKeySigner;
use clap::Parser;
use eyre::{eyre, Result};
use futures_util::StreamExt;
use reth_ethereum::{
    chainspec::ChainSpec,
    node::{
        builder::{NodeBuilder, NodeHandle},
        core::{args::RpcServerArgs, node_config::NodeConfig},
        EthereumNode,
    },
    provider::{CanonStateSubscriptions, DatabaseProviderFactory, HeaderProvider},
    rpc::api::eth::helpers::EthTransactions,
    tasks::TaskManager,
};

const CHAIN_ID: u64 = 2600;

#[derive(Debug, Parser)]
#[command(about = "Demonstrate engine persistence threshold behavior")]
struct Args {
    /// Run two scenarios: threshold 0 and the compare threshold.
    #[arg(long)]
    compare: bool,

    /// Persistence threshold to use when not running in compare mode.
    #[arg(long, default_value_t = 0)]
    persistence_threshold: u64,

    /// Threshold to use for the compare run.
    #[arg(long, default_value_t = 2)]
    compare_threshold: u64,

    /// Poll interval for checking on-disk visibility.
    #[arg(long, default_value_t = 100)]
    poll_interval_ms: u64,

    /// Timeout for waiting on disk visibility.
    #[arg(long, default_value_t = 5_000)]
    visible_timeout_ms: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let poll_interval = Duration::from_millis(args.poll_interval_ms);
    let visible_timeout = Duration::from_millis(args.visible_timeout_ms);

    if args.compare {
        run_scenario("threshold-0", 0, poll_interval, visible_timeout, true).await?;
        run_scenario(
            "threshold-compare",
            args.compare_threshold,
            poll_interval,
            visible_timeout,
            false,
        )
        .await?;
    } else {
        run_scenario(
            "single",
            args.persistence_threshold,
            poll_interval,
            visible_timeout,
            args.persistence_threshold == 0,
        )
        .await?;
    }

    Ok(())
}

async fn run_scenario(
    label: &str,
    persistence_threshold: u64,
    poll_interval: Duration,
    visible_timeout: Duration,
    expect_immediate: bool,
) -> Result<()> {
    println!("[{label}] starting with persistence_threshold={persistence_threshold}");

    // Keep a tempdir alive for the duration of the scenario so the DB stays on disk.
    let tempdir = tempfile::tempdir()?;
    let datadir = tempdir.path().to_path_buf();

    let signer = PrivateKeySigner::random();
    let signer_address = signer.address();
    let wallet = EthereumWallet::new(signer.clone());

    let chain = custom_chain(signer_address);

    let mut node_config = NodeConfig::test()
        .dev()
        .with_rpc(RpcServerArgs::default().with_http())
        .with_chain(chain.clone());

    node_config.engine.persistence_threshold = persistence_threshold;
    node_config.engine.memory_block_buffer_target = 0;

    let tasks = TaskManager::current();

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node_with_datadir(tasks.executor(), datadir.clone())
        .node(EthereumNode::default())
        .launch_with_debug_capabilities()
        .await?;

    let factory = node.provider.clone();

    let eth_api = node.rpc_registry.eth_api();
    let mut notifications = node.provider.canonical_state_stream();

    let mut nonce = 0u64;

    let first_hash =
        send_transfer(eth_api.clone(), &wallet, signer_address, nonce, CHAIN_ID).await?;
    nonce += 1;
    println!("[{label}] submitted transaction: {first_hash}");

    let head = next_head(&mut notifications).await?;
    let expected_block = head.tip().number;
    println!("[{label}] mined block: {expected_block}");

    let immediate_visible = {
        let provider = factory.database_provider_ro()?;
        provider.header_by_number(expected_block)?.is_some()
    };

    if expect_immediate {
        if immediate_visible {
            println!("[{label}] block {expected_block} visible immediately");
        } else {
            let waited =
                wait_until_visible(&factory, expected_block, poll_interval, visible_timeout)
                    .await?;
            println!("[{label}] block {expected_block} became visible after {waited:?}");
        }
        return Ok(());
    }

    if immediate_visible {
        println!(
            "[{label}] block {expected_block} visible immediately (unexpected for threshold > 0)"
        );
    } else {
        println!("[{label}] block {expected_block} not visible immediately (expected)");
    }

    // Produce additional blocks to cross the persistence threshold.
    for _ in 0..persistence_threshold {
        let _hash =
            send_transfer(eth_api.clone(), &wallet, signer_address, nonce, CHAIN_ID).await?;
        nonce += 1;
        let _ = next_head(&mut notifications).await?;
    }

    let waited =
        wait_until_visible(&factory, expected_block, poll_interval, visible_timeout).await?;
    println!(
        "[{label}] block {expected_block} became visible after {waited:?} once threshold exceeded"
    );

    Ok(())
}

async fn next_head(
    notifications: &mut (impl futures_util::Stream<Item = reth_ethereum::provider::CanonStateNotification>
              + Unpin),
) -> Result<reth_ethereum::provider::CanonStateNotification> {
    notifications.next().await.ok_or_else(|| eyre!("canonical head stream ended"))
}

async fn wait_until_visible(
    factory: &impl DatabaseProviderFactory<Provider: HeaderProvider>,
    block_number: u64,
    poll_interval: Duration,
    timeout: Duration,
) -> Result<Duration> {
    let start = tokio::time::Instant::now();
    loop {
        let provider = factory.database_provider_ro()?;
        if provider.header_by_number(block_number)?.is_some() {
            return Ok(start.elapsed());
        }

        if start.elapsed() > timeout {
            return Err(eyre!(
                "timed out waiting for block {block_number} to be persisted after {:?}",
                start.elapsed()
            ));
        }

        drop(provider);
        tokio::time::sleep(poll_interval).await;
    }
}

async fn send_transfer(
    eth_api: impl EthTransactions + Clone,
    wallet: &EthereumWallet,
    to: Address,
    nonce: u64,
    chain_id: u64,
) -> Result<B256> {
    let request = reth_ethereum::rpc::eth::primitives::TransactionRequest::default()
        .with_to(to)
        .with_value(U256::from(1))
        .with_nonce(nonce)
        .with_chain_id(chain_id)
        .with_gas_limit(21_000)
        .with_max_priority_fee_per_gas(1_000_000_000u128)
        .with_max_fee_per_gas(1_000_000_000u128);

    let signed: reth_ethereum::TransactionSigned =
        NetworkWallet::<Ethereum>::sign_request(wallet, request)
            .await
            .map(Into::into)
            .map_err(|err| eyre!("sign request failed: {err:?}"))?;
    let raw: Bytes = signed.encoded_2718().into();

    eth_api
        .send_raw_transaction(raw)
        .await
        .map_err(|err| eyre!("send_raw_transaction failed: {err:?}"))
}

fn custom_chain(signer_address: Address) -> Arc<ChainSpec> {
    let custom_genesis = r#"
{
    "nonce": "0x42",
    "timestamp": "0x0",
    "extraData": "0x5343",
    "gasLimit": "0x5208",
    "difficulty": "0x400000000",
    "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "coinbase": "0x0000000000000000000000000000000000000000",
    "alloc": {},
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
    let mut genesis: Genesis = serde_json::from_str(custom_genesis).unwrap();
    let balance = U256::from(10).pow(U256::from(18));
    genesis.alloc.insert(signer_address, GenesisAccount::default().with_balance(balance));
    Arc::new(genesis.into())
}
