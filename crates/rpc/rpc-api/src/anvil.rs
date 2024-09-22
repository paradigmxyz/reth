use jsonrpsee::{core::RpcResult, proc_macros::rpc};

use alloy_primitives::{Address, Bytes, B256, U256};
use alloy_rpc_types::Block;
use alloy_rpc_types_anvil::{Forking, Metadata, MineOptions, NodeInfo};

/// Anvil rpc interface.
/// https://book.getfoundry.sh/reference/anvil/#custom-methods
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "anvil"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "anvil"))]
pub trait AnvilApi {
    /// Sends transactions impersonating specific account and contract addresses.
    #[method(name = "impersonateAccount")]
    async fn anvil_impersonate_account(&self, address: Address) -> RpcResult<()>;

    /// Stops impersonating an account if previously set with `anvil_impersonateAccount`.
    #[method(name = "stopImpersonatingAccount")]
    async fn anvil_stop_impersonating_account(&self, address: Address) -> RpcResult<()>;

    /// If set to true will make every account impersonated.
    #[method(name = "autoImpersonateAccount")]
    async fn anvil_auto_impersonate_account(&self, enabled: bool) -> RpcResult<()>;

    /// Returns `true` if auto mining is enabled, and `false`.
    #[method(name = "getAutomine")]
    async fn anvil_get_automine(&self) -> RpcResult<bool>;

    /// Mines a series of blocks.
    #[method(name = "mine")]
    async fn anvil_mine(&self, blocks: Option<U256>, interval: Option<U256>) -> RpcResult<()>;

    /// Enables or disables, based on the single boolean argument, the automatic mining of new
    /// blocks with each new transaction submitted to the network.
    #[method(name = "setAutomine")]
    async fn anvil_set_automine(&self, enabled: bool) -> RpcResult<()>;

    /// Sets the mining behavior to interval with the given interval (seconds).
    #[method(name = "setIntervalMining")]
    async fn anvil_set_interval_mining(&self, interval: u64) -> RpcResult<()>;

    /// Removes transactions from the pool.
    #[method(name = "anvil_dropTransaction")]
    async fn anvil_drop_transaction(&self, tx_hash: B256) -> RpcResult<Option<B256>>;

    /// Resets the fork to a fresh forked state, and optionally update the fork config.
    ///
    /// If `forking` is `None` then this will disable forking entirely.
    #[method(name = "reset")]
    async fn anvil_reset(&self, fork: Option<Forking>) -> RpcResult<()>;

    /// Sets the backend rpc url.
    #[method(name = "setRpcUrl")]
    async fn anvil_set_rpc_url(&self, url: String) -> RpcResult<()>;

    /// Modifies the balance of an account.
    #[method(name = "setBalance")]
    async fn anvil_set_balance(&self, address: Address, balance: U256) -> RpcResult<()>;

    /// Sets the code of a contract.
    #[method(name = "setCode")]
    async fn anvil_set_code(&self, address: Address, code: Bytes) -> RpcResult<()>;

    /// Sets the nonce of an address.
    #[method(name = "setNonce")]
    async fn anvil_set_nonce(&self, address: Address, nonce: U256) -> RpcResult<()>;

    /// Writes a single slot of the account's storage.
    #[method(name = "setStorageAt")]
    async fn anvil_set_storage_at(
        &self,
        address: Address,
        slot: U256,
        value: B256,
    ) -> RpcResult<bool>;

    /// Sets the coinbase address.
    #[method(name = "setCoinbase")]
    async fn anvil_set_coinbase(&self, address: Address) -> RpcResult<()>;

    /// Sets the chain id.
    #[method(name = "setChainId")]
    async fn anvil_set_chain_id(&self, chain_id: u64) -> RpcResult<()>;

    /// Enables or disable logging.
    #[method(name = "setLoggingEnabled")]
    async fn anvil_set_logging_enabled(&self, enabled: bool) -> RpcResult<()>;

    ///  Sets the minimum gas price for the node.
    #[method(name = "setMinGasPrice")]
    async fn anvil_set_min_gas_price(&self, gas_price: U256) -> RpcResult<()>;

    /// Sets the base fee of the next block.
    #[method(name = "setNextBlockBaseFeePerGas")]
    async fn anvil_set_next_block_base_fee_per_gas(&self, base_fee: U256) -> RpcResult<()>;

    /// Sets the minimum gas price for the node.
    #[method(name = "setTime")]
    async fn anvil_set_time(&self, timestamp: u64) -> RpcResult<u64>;

    /// Creates a buffer that represents all state on the chain, which can be loaded to separate
    /// process by calling `anvil_loadState`.
    #[method(name = "dumpState")]
    async fn anvil_dump_state(&self) -> RpcResult<Bytes>;

    /// Append chain state buffer to current chain.Will overwrite any conflicting addresses or
    /// storage.
    #[method(name = "loadState")]
    async fn anvil_load_state(&self, state: Bytes) -> RpcResult<bool>;

    /// Retrieves the Anvil node configuration params.
    #[method(name = "nodeInfo")]
    async fn anvil_node_info(&self) -> RpcResult<NodeInfo>;

    /// Retrieves metadata about the Anvil instance.
    #[method(name = "metadata")]
    async fn anvil_metadata(&self) -> RpcResult<Metadata>;

    /// Snapshot the state of the blockchain at the current block.
    #[method(name = "snapshot")]
    async fn anvil_snapshot(&self) -> RpcResult<U256>;

    /// Revert the state of the blockchain to a previous snapshot.
    /// Takes a single parameter, which is the snapshot id to revert to.
    #[method(name = "revert")]
    async fn anvil_revert(&self, id: U256) -> RpcResult<bool>;

    /// Jump forward in time by the given amount of time, in seconds.
    #[method(name = "increaseTime")]
    async fn anvil_increase_time(&self, seconds: U256) -> RpcResult<i64>;

    /// Similar to `evm_increaseTime` but takes the exact timestamp that you want in the next block.
    #[method(name = "setNextBlockTimestamp")]
    async fn anvil_set_next_block_timestamp(&self, seconds: u64) -> RpcResult<()>;

    /// Sets the next block gas limit.
    #[method(name = "setBlockGasLimit")]
    async fn anvil_set_block_gas_limit(&self, gas_limit: U256) -> RpcResult<bool>;

    /// Sets an interval for the block timestamp.
    #[method(name = "setBlockTimestampInterval")]
    async fn anvil_set_block_timestamp_interval(&self, seconds: u64) -> RpcResult<()>;

    /// Sets an interval for the block timestamp.
    #[method(name = "removeBlockTimestampInterval")]
    async fn anvil_remove_block_timestamp_interval(&self) -> RpcResult<bool>;

    /// Mine blocks, instantly and return the mined blocks.
    ///
    /// This will mine the blocks regardless of the configured mining mode.
    ///
    /// **Note**: This behaves exactly as `evm_mine` but returns different output, for
    /// compatibility reasons, this is a separate call since `evm_mine` is not an anvil original.
    /// and `ganache` may change the `0x0` placeholder.
    #[method(name = "mine_detailed")] // This method requires using `snake_case`.
    async fn anvil_mine_detailed(&self, opts: Option<MineOptions>) -> RpcResult<Vec<Block>>;

    /// Turn on call traces for transactions that are returned to the user when they execute a
    /// transaction (instead of just txhash/receipt).
    #[method(name = "enableTraces")]
    async fn anvil_enable_traces(&self) -> RpcResult<()>;

    /// Removes all transactions for that address from the transaction pool.
    #[method(name = "removePoolTransactions")]
    async fn anvil_remove_pool_transactions(&self, address: Address) -> RpcResult<()>;
}
