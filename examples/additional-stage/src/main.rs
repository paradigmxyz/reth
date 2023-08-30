//! Example of how to add a custom index to Reth. The index is populated by a custom stage,
//! and can be queried by a custom JSON-RPC endpoint.
//!
//! Run with
//!
//! ```not_rust
//! cargo run -p additional-stage -- node --http --ws --enable-mynamespace
//! ```
//!
//! This example features an additional:
//! - stage (./stage.rs)
//! - database table (./table.rs)
//! - namespace
//!
//! The example shows Reth being started with normal functionality, but with an additional features:
//! - `MyStage`, that executes after native Reth stages.
//! - `MyTable`, that `MyStage` uses to store data.
//! - `mynamespace_myMethod`, that uses MyTable data to respond to JSON-RPC requests.
//!
//! This installs an additional RPC method `mynamespace_myMethod` that can queried via [cast](https://github.com/foundry-rs/foundry)
//!
//! ```sh
//! cast rpc mynamespace_myMethod
//! ```
//!
//! Specifically, the example adds a new stage that records the most recent block that a miner
//! produced. A new table stores this data. The JSON-RPC method returns the latest block produced
//! (if any) for any given address.
use std::sync::Arc;

use clap::Parser;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth::{
    cli::{
        config::RethRpcConfig,
        ext::{NoAdditionalTablesConfig, RethCliExt, RethNodeCommandConfig},
        Cli,
    },
    db,
    network::{NetworkInfo, Peers},
    providers::{
        BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider, ChangeSetReader,
        EvmEnvProvider, StateProviderFactory,
    },
    rpc::builder::{RethModuleRegistry, TransportRpcModules},
    tasks::TaskSpawner,
};
use reth_db::{database::Database, transaction::DbTx, TableMetadata};
use reth_primitives::{Address, BlockNumber};
use reth_stages::{PipelineBuilder, StageSet};
use reth_transaction_pool::TransactionPool;
use stage::{MyStage, MyStageSet};
use table::{MyTable, NonCoreTable};

// Custom modules for the example.
mod stage;
mod table;

fn main() {
    // Start reth
    Cli::<MyRethCliExt>::parse().run().unwrap();
}

/// The type that tells the reth CLI what extensions to use
struct MyRethCliExt;

impl RethCliExt for MyRethCliExt {
    /// This tells the reth CLI to install the `mynamespace` rpc namespace via `RethCliNamespaceExt`
    type Node = RethCliNamespaceExt;
    /// This tells the reth CLI to use additional non-core tables.
    type TableExt = NonCoreTable;
}

/// Our custom cli args extension that adds one flag to reth default CLI.
#[derive(Debug, Clone, Copy, Default, clap::Args)]
struct RethCliNamespaceExt {
    /// CLI flag to enable the mynamespace extension namespace
    #[clap(long)]
    pub enable_mynamespace: bool,
}

// Note: For all the methods in this trait, the method arguments are provided by Reth.
impl RethNodeCommandConfig for RethCliNamespaceExt {
    type TableExt = NonCoreTable;

    // This is the entrypoint for the CLI to extend the RPC server with custom rpc namespaces.
    fn extend_rpc_modules<Conf, Provider, Pool, Network, Tasks, Events>(
        &mut self,
        _config: &Conf,
        registry: &mut RethModuleRegistry<Provider, Pool, Network, Tasks, Events>,
        modules: &mut TransportRpcModules,
    ) -> eyre::Result<()>
    where
        Conf: RethRpcConfig,
        Provider: BlockReaderIdExt
            + StateProviderFactory
            + EvmEnvProvider
            + ChainSpecProvider
            + ChangeSetReader
            + Clone
            + Unpin
            + 'static,
        Pool: TransactionPool + Clone + 'static,
        Network: NetworkInfo + Peers + Clone + 'static,
        Tasks: TaskSpawner + Clone + 'static,
        Events: CanonStateSubscriptions + Clone + 'static,
    {
        if !self.enable_mynamespace {
            return Ok(())
        }

        let database = todo!("Get reference to database");
        let ext = MyNamespace { database };

        // now we merge our extension namespace into all configured transports
        modules.merge_configured(ext.into_rpc())?;

        println!("mynamespace extension enabled");
        Ok(())
    }

    fn add_custom_stage<'a, DB>(
        &self,
        pipeline_builder: &'a mut PipelineBuilder<DB>,
    ) -> eyre::Result<&'a mut PipelineBuilder<DB>>
    where
        DB: Database,
    {
        // Make a new set of stages.
        let set = MyStageSet::new().set(MyStage::new());
        // Add set to pipeline.
        pipeline_builder.add_stages(set);
        Ok(pipeline_builder)
    }
}

/// trait interface for a custom rpc namespace: `mynamespace`
///
/// This defines an additional namespace where all methods are configured as trait functions.
#[rpc(server, namespace = "MyNamespace")]
pub trait MyNamespaceApi {
    /// Returns the response to mynamespace_myMethod
    #[method(name = "myMethod")]
    fn my_method(&self, my_param: Address) -> RpcResult<MyMethodResponse>;
}

/// The type that implements the `mynamespace` rpc namespace trait
///
/// Usage: Members of this struct hold data for constructing the response.
#[derive(Debug, Clone)]
pub struct MyNamespace<'a, DB: Database> {
    /// A reference to the reth database. Used by MyNamespace to construct responses.
    database: &'a DatabaseProviderRO<'a, &'a DB>,
}

impl<DB: Database> MyNamespaceApiServer for MyNamespace<'static, DB> {
    fn my_method(&self, my_param: Address) -> RpcResult<MyMethodResponse> {
        let response: Option<BlockNumber> = self
            .database
            .tx_ref()
            .get::<MyTable>(my_param)
            .expect("Couldn't read MyTable");
        Ok(response)
    }
}

/// Data for the response to the JSON-RPC method.
type MyMethodResponse = Option<BlockNumber>;
