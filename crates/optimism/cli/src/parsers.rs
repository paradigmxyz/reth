//! Optimism CLI parsers implementation.

use crate::chainspec::OpChainSpecParser;
use reth_cli_util::RethCliParsers;
use reth_rpc_server_types::DefaultRpcModuleValidator;

/// Default Optimism CLI parsers with strict RPC module validation.
#[derive(Debug, Clone, Copy)]
pub struct OpCliParsers;

impl RethCliParsers for OpCliParsers {
    type ChainSpecParser = OpChainSpecParser;
    type RpcModuleValidator = DefaultRpcModuleValidator;
}
