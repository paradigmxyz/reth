use crate::parsers::OpCliParsers;
use clap::Subcommand;
use import::ImportOpCommand;
use import_receipts::ImportReceiptsOpCommand;
use reth_chainspec::{EthChainSpec, EthereumHardforks, Hardforks};
use reth_cli_commands::{
    config_cmd, db, dump_genesis, init_cmd,
    node::{self, NoArgs},
    p2p, prune, re_execute, recover, stage,
};
use reth_cli_util::RethCliParsers;
use std::{fmt, sync::Arc};

pub mod import;
pub mod import_receipts;
pub mod init_state;

#[cfg(feature = "dev")]
pub mod test_vectors;

/// Commands to be executed
#[derive(Debug, Subcommand)]
pub enum Commands<P: RethCliParsers = OpCliParsers, Ext: clap::Args + fmt::Debug = NoArgs> {
    /// Start the node
    #[command(name = "node")]
    Node(Box<node::NodeCommand<P::ChainSpecParser, Ext>>),
    /// Initialize the database from a genesis file.
    #[command(name = "init")]
    Init(init_cmd::InitCommand<P::ChainSpecParser>),
    /// Initialize the database from a state dump file.
    #[command(name = "init-state")]
    InitState(init_state::InitStateCommandOp<P::ChainSpecParser>),
    /// This syncs RLP encoded OP blocks below Bedrock from a file, without executing.
    #[command(name = "import-op")]
    ImportOp(ImportOpCommand<P::ChainSpecParser>),
    /// This imports RLP encoded receipts from a file.
    #[command(name = "import-receipts-op")]
    ImportReceiptsOp(ImportReceiptsOpCommand<P::ChainSpecParser>),
    /// Dumps genesis block JSON configuration to stdout.
    DumpGenesis(dump_genesis::DumpGenesisCommand<P::ChainSpecParser>),
    /// Database debugging utilities
    #[command(name = "db")]
    Db(db::Command<P::ChainSpecParser>),
    /// Manipulate individual stages.
    #[command(name = "stage")]
    Stage(Box<stage::Command<P::ChainSpecParser>>),
    /// P2P Debugging utilities
    #[command(name = "p2p")]
    P2P(p2p::Command<P::ChainSpecParser>),
    /// Write config to stdout
    #[command(name = "config")]
    Config(config_cmd::Command),
    /// Scripts for node recovery
    #[command(name = "recover")]
    Recover(recover::Command<P::ChainSpecParser>),
    /// Prune according to the configuration without any limits
    #[command(name = "prune")]
    Prune(prune::PruneCommand<P::ChainSpecParser>),
    /// Generate Test Vectors
    #[cfg(feature = "dev")]
    #[command(name = "test-vectors")]
    TestVectors(test_vectors::Command),
    /// Re-execute blocks in parallel to verify historical sync correctness.
    #[command(name = "re-execute")]
    ReExecute(re_execute::Command<P::ChainSpecParser>),
}

impl<P: RethCliParsers, Ext: clap::Args + fmt::Debug> Commands<P, Ext>
where
    P::ChainSpecParser: reth_cli::chainspec::ChainSpecParser<
        ChainSpec: EthChainSpec + Hardforks + EthereumHardforks,
    >,
{
    /// Returns the underlying chain being used for commands
    pub fn chain_spec(
        &self,
    ) -> Option<&Arc<<P::ChainSpecParser as reth_cli::chainspec::ChainSpecParser>::ChainSpec>> {
        match self {
            Self::Node(cmd) => cmd.chain_spec(),
            Self::Init(cmd) => cmd.chain_spec(),
            Self::InitState(cmd) => cmd.chain_spec(),
            Self::DumpGenesis(cmd) => cmd.chain_spec(),
            Self::Db(cmd) => cmd.chain_spec(),
            Self::Stage(cmd) => cmd.chain_spec(),
            Self::P2P(cmd) => cmd.chain_spec(),
            Self::Config(_) => None,
            Self::Recover(cmd) => cmd.chain_spec(),
            Self::Prune(cmd) => cmd.chain_spec(),
            Self::ImportOp(cmd) => cmd.chain_spec(),
            Self::ImportReceiptsOp(cmd) => cmd.chain_spec(),
            #[cfg(feature = "dev")]
            Self::TestVectors(_) => None,
            Self::ReExecute(cmd) => cmd.chain_spec(),
        }
    }
}
