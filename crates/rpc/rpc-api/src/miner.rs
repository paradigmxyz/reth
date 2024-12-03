use alloy_primitives::{Bytes, U128};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};

/// Miner namespace rpc interface that can control miner/builder settings
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "miner"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "miner"))]
pub trait MinerApi {
    /// Sets the extra data string that is included when this miner mines a block.
    ///
    /// Returns an error if the extra data is too long.
    #[method(name = "setExtra")]
    fn set_extra(&self, record: Bytes) -> RpcResult<bool>;

    /// Sets the minimum accepted gas price for the miner.
    #[method(name = "setGasPrice")]
    fn set_gas_price(&self, gas_price: U128) -> RpcResult<bool>;

    /// Sets the gaslimit to target towards during mining.
    #[method(name = "setGasLimit")]
    fn set_gas_limit(&self, gas_price: U128) -> RpcResult<bool>;
}
