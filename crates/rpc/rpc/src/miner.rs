//! Implementation of the [`MinerApiServer`] trait.
//!
//! The `miner_` namespace was introduced for Ethereum's proof-of-work era. Since the
//! Merge (Paris upgrade) Ethereum uses proof-of-stake consensus, so these methods are
//! no-ops retained only for tooling compatibility. They always return `false`.

use alloy_primitives::{Bytes, U128};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_rpc_api::MinerApiServer;

/// `miner` API implementation.
///
/// This type provides the functionality for handling `miner` related requests.
/// All methods are no-ops since Ethereum transitioned to proof-of-stake (Paris
/// upgrade) and mining no longer applies.
#[derive(Clone, Debug, Default)]
pub struct MinerApi {}

#[async_trait]
impl MinerApiServer for MinerApi {
    /// Handler for `miner_setExtra`
    ///
    /// Sets the extra data string included when a miner mines a block.
    /// Always returns `false` because reth does not mine blocks (PoS).
    fn set_extra(&self, _record: Bytes) -> RpcResult<bool> {
        Ok(false)
    }

    /// Handler for `miner_setGasPrice`
    ///
    /// Sets the minimum accepted gas price for the miner.
    /// Always returns `false` because reth does not mine blocks (PoS).
    fn set_gas_price(&self, _gas_price: U128) -> RpcResult<bool> {
        Ok(false)
    }

    /// Handler for `miner_setGasLimit`
    ///
    /// Sets the gas limit to target during mining.
    /// Always returns `false` because reth does not mine blocks (PoS).
    fn set_gas_limit(&self, _gas_limit: U128) -> RpcResult<bool> {
        Ok(false)
    }
}
