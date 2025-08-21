//! Ethereum CLI parsers implementation.

use crate::chainspec::EthereumChainSpecParser;
use reth_cli_util::RethCliParsers;
use reth_rpc_server_types::DefaultRpcModuleValidator;

/// Default Ethereum CLI parsers with strict RPC module validation.
#[derive(Debug, Clone, Copy)]
pub struct EthereumCliParsers;

impl RethCliParsers for EthereumCliParsers {
    type ChainSpecParser = EthereumChainSpecParser;
    type RpcModuleValidator = DefaultRpcModuleValidator;
}
