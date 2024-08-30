use crate::chainspec::OpChainSpecParser;
use clap::Subcommand;
use import::ImportOpCommand;
use import_receipts::ImportReceiptsOpCommand;
use reth_cli_commands::{
    config_cmd, db, dump_genesis, init_cmd, init_state,
    node::{self, NoArgs},
    p2p, prune, recover, stage,
};
use std::fmt;

/// Helper function to build an import pipeline.
mod build_pipeline;
pub mod import;
pub mod import_receipts;

/// Commands to be executed
#[derive(Debug, Subcommand)]
pub enum Commands<Ext: clap::Args + fmt::Debug = NoArgs> {
    /// Start the node
    #[command(name = "node")]
    Node(Box<node::NodeCommand<OpChainSpecParser, Ext>>),
    /// Initialize the database from a genesis file.
    #[command(name = "init")]
    Init(init_cmd::InitCommand<OpChainSpecParser>),
    /// Initialize the database from a state dump file.
    #[command(name = "init-state")]
    InitState(init_state::InitStateCommand<OpChainSpecParser>),
    /// This syncs RLP encoded OP blocks below Bedrock from a file, without executing.
    #[command(name = "import-op")]
    ImportOp(ImportOpCommand<OpChainSpecParser>),
    /// This imports RLP encoded receipts from a file.
    #[command(name = "import-receipts-op")]
    ImportReceiptsOp(ImportReceiptsOpCommand<OpChainSpecParser>),
    /// Dumps genesis block JSON configuration to stdout.
    DumpGenesis(dump_genesis::DumpGenesisCommand<OpChainSpecParser>),
    /// Database debugging utilities
    #[command(name = "db")]
    Db(db::Command<OpChainSpecParser>),
    /// Manipulate individual stages.
    #[command(name = "stage")]
    Stage(Box<stage::Command<OpChainSpecParser>>),
    /// P2P Debugging utilities
    #[command(name = "p2p")]
    P2P(p2p::Command<OpChainSpecParser>),
    /// Write config to stdout
    #[command(name = "config")]
    Config(config_cmd::Command),
    /// Scripts for node recovery
    #[command(name = "recover")]
    Recover(recover::Command<OpChainSpecParser>),
    /// Prune according to the configuration without any limits
    #[command(name = "prune")]
    Prune(prune::PruneCommand<OpChainSpecParser>),
}
