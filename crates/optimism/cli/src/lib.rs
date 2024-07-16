//! OP-Reth CLI implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(all(not(test), feature = "optimism"), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
// The `optimism` feature must be enabled to use this crate.
#![cfg(feature = "optimism")]

/// Optimism chain specification parser.
pub mod chainspec;
/// Optimism CLI commands.
pub mod commands;
/// Module with a codec for reading and encoding receipts in files.
///
/// Enables decoding and encoding `HackReceipt` type. See <https://github.com/testinprod-io/op-geth/pull/1>.
///
/// Currently configured to use codec [`HackReceipt`](file_codec_ovm_receipt::HackReceipt) based on
/// export of below Bedrock data using <https://github.com/testinprod-io/op-geth/pull/1>. Codec can
/// be replaced with regular encoding of receipts for export.
///
/// NOTE: receipts can be exported using regular op-geth encoding for `Receipt` type, to fit
/// reth's needs for importing. However, this would require patching the diff in <https://github.com/testinprod-io/op-geth/pull/1> to export the `Receipt` and not `HackReceipt` type (originally
/// made for op-erigon's import needs).
pub mod file_codec_ovm_receipt;

pub use commands::{import::ImportOpCommand, import_receipts::ImportReceiptsOpCommand};

use std::{ffi::OsString, fmt, sync::Arc};

use chainspec::OpChainSpecParser;
use clap::{command, value_parser, Parser};
use commands::Commands;
use reth_chainspec::ChainSpec;
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::node::NoArgs;
use reth_node_core::{
    args::{utils::chain_help, LogArgs},
    version::{LONG_VERSION, SHORT_VERSION},
};

/// The main reth cli interface.
///
/// This is the entrypoint to the executable.
#[derive(Debug, Parser)]
#[command(author, version = SHORT_VERSION, long_version = LONG_VERSION, about = "Reth", long_about = None)]
pub struct Cli<Ext: clap::Args + fmt::Debug = NoArgs> {
    /// The command to run
    #[command(subcommand)]
    command: Commands<Ext>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = OpChainSpecParser::SUPPORTED_CHAINS[0],
        value_parser = OpChainSpecParser::default(),
        global = true,
    )]
    chain: Arc<ChainSpec>,

    /// Add a new instance of a node.
    ///
    /// Configures the ports of the node to avoid conflicts with the defaults.
    /// This is useful for running multiple nodes on the same machine.
    ///
    /// Max number of instances is 200. It is chosen in a way so that it's not possible to have
    /// port numbers that conflict with each other.
    ///
    /// Changes to the following port numbers:
    /// - `DISCOVERY_PORT`: default + `instance` - 1
    /// - `AUTH_PORT`: default + `instance` * 100 - 100
    /// - `HTTP_RPC_PORT`: default - `instance` + 1
    /// - `WS_RPC_PORT`: default + `instance` * 2 - 2
    #[arg(long, value_name = "INSTANCE", global = true, default_value_t = 1, value_parser = value_parser!(u16).range(..=200))]
    instance: u16,

    #[command(flatten)]
    logs: LogArgs,
}

impl Cli {
    /// Parsers only the default CLI arguments
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Parsers only the default CLI arguments from the given iterator
    pub fn try_parse_args_from<I, T>(itr: I) -> Result<Self, clap::error::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        Self::try_parse_from(itr)
    }
}
