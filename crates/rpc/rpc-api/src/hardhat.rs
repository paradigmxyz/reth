use alloy_primitives::{Address, Bytes, B256, U256};
use alloy_rpc_types_anvil::{Forking, Metadata};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};

/// Hardhat rpc interface.
/// https://hardhat.org/hardhat-network/docs/reference#hardhat-network-methods
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "hardhat"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "hardhat"))]
pub trait HardhatApi {
    /// Removes the given transaction from the mempool, if it exists.
    ///
    /// Returns `true` if successful, otherwise `false`.
    #[method(name = "hardhat_dropTransaction")]
    async fn hardhat_drop_transaction(&self, tx_hash: B256) -> RpcResult<bool>;

    /// Allows Hardhat Network to sign transactions as the given address.
    #[method(name = "impersonateAccount")]
    async fn hardhat_impersonate_account(&self, address: Address) -> RpcResult<()>;

    /// Returns `true` if automatic mining is enabled, and `false` otherwise.
    #[method(name = "getAutomine")]
    async fn hardhat_get_automine(&self) -> RpcResult<bool>;

    /// Returns an object with metadata about the instance of the Hardhat network.
    #[method(name = "metadata")]
    async fn hardhat_metadata(&self) -> RpcResult<Metadata>;

    /// Mines a specified number of blocks at a given interval.
    #[method(name = "mine")]
    async fn hardhat_mine(&self, blocks: Option<U256>, interval: Option<U256>) -> RpcResult<()>;

    /// Resets back to a fresh forked state, fork from another block number or disable forking.
    #[method(name = "reset")]
    async fn hardhat_reset(&self, fork: Option<Forking>) -> RpcResult<()>;

    /// Sets the balance for the given address.
    #[method(name = "setBalance")]
    async fn hardhat_set_balance(&self, address: Address, balance: U256) -> RpcResult<()>;

    /// Modifies the bytecode stored at an account's address.
    #[method(name = "setCode")]
    async fn hardhat_set_code(&self, address: Address, code: Bytes) -> RpcResult<()>;

    /// Sets the coinbase address to be used in new blocks.
    #[method(name = "setCoinbase")]
    async fn hardhat_set_coinbase(&self, address: Address) -> RpcResult<()>;

    /// Enables or disables logging.
    #[method(name = "setLoggingEnabled")]
    async fn hardhat_set_logging_enabled(&self, enabled: bool) -> RpcResult<()>;

    /// Changes the minimum gas price accepted by the network (in wei).
    #[method(name = "setMinGasPrice")]
    async fn hardhat_set_min_gas_price(&self, gas_price: U256) -> RpcResult<()>;

    /// Sets the base fee of the next block.
    #[method(name = "setNextBlockBaseFeePerGas")]
    async fn hardhat_set_next_block_base_fee_per_gas(
        &self,
        base_fee_per_gas: U256,
    ) -> RpcResult<()>;

    /// Sets the `PREVRANDAO` value of the next block.
    #[method(name = "setPrevRandao")]
    async fn hardhat_set_prev_randao(&self, prev_randao: B256) -> RpcResult<()>;

    /// Modifies an account's nonce by overwriting it.
    #[method(name = "setNonce")]
    async fn hardhat_set_nonce(&self, address: Address, nonce: U256) -> RpcResult<()>;

    /// Writes a single position of an account's storage.
    #[method(name = "setStorageAt")]
    async fn hardhat_set_storage_at(
        &self,
        address: Address,
        slot: U256,
        value: B256,
    ) -> RpcResult<()>;

    /// Stops impersonating the given address.
    #[method(name = "stopImpersonatingAccount")]
    async fn hardhat_stop_impersonating_account(&self, address: Address) -> RpcResult<()>;
}
