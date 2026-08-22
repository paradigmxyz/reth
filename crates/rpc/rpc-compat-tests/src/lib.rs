//! Rust-native execution-apis RPC compatibility testing for Reth.

#[cfg(feature = "embedded")]
use alloy_genesis::Genesis;
#[cfg(feature = "embedded")]
use eyre::{eyre, Context, Result};
#[cfg(feature = "embedded")]
use reth_chainspec::ChainSpec;
#[cfg(feature = "embedded")]
use reth_e2e_test_utils::testsuite::{
    actions::{MakeCanonical, UpdateBlockInfo},
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
#[cfg(feature = "embedded")]
use reth_node_core::args::DefaultRpcServerArgs;
#[cfg(feature = "embedded")]
use reth_node_ethereum::{EthEngineTypes, EthereumNode};
#[cfg(feature = "embedded")]
use std::{fs, sync::Arc};

pub mod case;
pub mod config;
pub mod fixture;
pub mod matcher;
pub mod report;
#[cfg(feature = "embedded")]
pub mod runner;
pub mod schema;

/// Launches an embedded Reth node and executes the resolved compatibility suite.
#[cfg(feature = "embedded")]
pub async fn run_embedded(
    fixture: fixture::Fixture,
    config: config::ResolvedRunConfig,
) -> Result<()> {
    DefaultRpcServerArgs::default()
        .with_rpc_compute_state_root_for_eth_simulate(true)
        .try_init()
        .map_err(|_| eyre!("RPC server defaults were initialized before the compatibility runner"))?;
    let genesis_path = fixture.tests.join("genesis.json");
    let genesis: Genesis = serde_json::from_str(&fs::read_to_string(&genesis_path)?)
        .wrap_err_with(|| format!("failed to parse {}", genesis_path.display()))?;
    let chain_spec = Arc::new(ChainSpec::from(genesis));
    let schemas = schema::SchemaCatalog::load(&fixture.root)?;
    let setup = Setup::<EthEngineTypes>::default()
        .with_chain_spec(chain_spec)
        .with_network(NetworkSetup::single_node());
    let fcu = fixture.tests.join("headfcu.json");
    let chain = fixture.tests.join("chain.rlp");

    TestBuilder::new()
        .with_setup_and_import(setup, chain)
        .with_action(UpdateBlockInfo::default())
        .with_action(runner::InitializeFixture::new(fcu.to_string_lossy()))
        .with_action(MakeCanonical::new())
        .with_action(runner::RunRpcCompatTests::new(fixture, config, schemas))
        .run::<EthereumNode>()
        .await
}
