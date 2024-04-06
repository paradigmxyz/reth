use jsonrpsee::{core::RpcResult, proc_macros::rpc};

use reth_primitives::{Address, B256, Bytes, U256};
use reth_rpc_types::{anvil::{Forking, Params}, Block};
use reth_rpc_types::anvil::{EvmMineOptions, Metadata, NodeInfo};

/// Anvil rpc interface.
/// https://book.getfoundry.sh/reference/anvil/#custom-methods
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "anvil"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "anvil"))]
pub trait AnvilApi {
    #[method(name = "impersonateAccount")]
    async fn anvil_impersonate_account(&self, address: Address) -> RpcResult<()>;

    #[method(name = "stopImpersonatingAccount")]
    async fn anvil_stop_impersonating_account(&self, address: Address) -> RpcResult<()>;

    #[method(name = "autoImpersonateAccount")]
    async fn anvil_auto_impersonate_account(&self, enabled: bool) -> RpcResult<()>;

    #[method(name = "getAutomine")]
    async fn anvil_get_automine(&self) -> RpcResult<bool>;

    #[method(name = "mine")]
    async fn anvil_mine(&self, blocks: Option<U256>, interval: Option<U256>) -> RpcResult<()>;

    #[method(name = "setAutomine")]
    async fn anvil_set_automine(&self, enabled: bool) -> RpcResult<()>;

    #[method(name = "setIntervalMining")]
    async fn anvil_set_interval_mining(&self, interval: u64) -> RpcResult<()>;

    #[method(name = "anvil_dropTransaction")]
    async fn anvil_drop_transaction(&self, tx_hash: B256) -> RpcResult<Option<B256>>;

    #[method(name = "reset")]
    async fn anvil_reset(&self, fork: Option<Params<Option<Forking>>>) -> RpcResult<()>;

    #[method(name = "setRpcUrl")]
    async fn anvil_set_rpc_url(&self, url: String) -> RpcResult<()>;

    #[method(name = "setBalance")]
    async fn anvil_set_balance(&self, address: Address, balance: U256) -> RpcResult<()>;

    #[method(name = "setCode")]
    async fn anvil_set_code(&self, address: Address, code: Bytes) -> RpcResult<()>;

    #[method(name = "setNonce")]
    async fn anvil_set_nonce(&self, address: Address, nonce: U256) -> RpcResult<()>;

    #[method(name = "setStorageAt")]
    async fn anvil_set_storage_at(&self, address: Address, slot: U256, value: B256) -> RpcResult<bool>;

    #[method(name = "setCoinbase")]
    async fn anvil_set_coinbase(&self, address: Address) -> RpcResult<()>;

    #[method(name = "setChainId")]
    async fn anvil_set_chain_id(&self, chain_id: u64) -> RpcResult<()>;

    #[method(name = "setLoggingEnabled")]
    async fn anvil_set_logging_enabled(&self, enabled: bool) -> RpcResult<()>;

    #[method(name = "setMinGasPrice")]
    async fn anvil_set_min_gas_price(&self, gas_price: U256) -> RpcResult<()>;

    #[method(name = "setNextBlockBaseFeePerGas")]
    async fn anvil_set_next_block_base_fee_per_gas(&self, base_fee: U256) -> RpcResult<()>;

    #[method(name = "setTime")]
    async fn anvil_set_time(&self, timestamp: u64) -> RpcResult<u64>;

    #[method(name = "dumpState")]
    async fn anvil_dump_state(&self) -> RpcResult<Bytes>;

    #[method(name = "loadState")]
    async fn anvil_load_state(&self, state: Bytes) -> RpcResult<bool>;

    #[method(name = "nodeInfo")]
    async fn anvil_node_info(&self) -> RpcResult<NodeInfo>;

    #[method(name = "metadata")]
    async fn anvil_metadata(&self) -> RpcResult<Metadata>;

    #[method(name = "snapshot")]
    async fn anvil_snapshot(&self) -> RpcResult<U256>;

    #[method(name = "revert")]
    async fn anvil_revert(&self, id: U256) -> RpcResult<bool>;

    #[method(name = "increaseTime")]
    async fn anvil_increase_time(&self, seconds: U256) -> RpcResult<i64>;

    #[method(name = "setNextBlockTimestamp")]
    async fn anvil_set_next_block_timestamp(&self, seconds: u64) -> RpcResult<()>;

    #[method(name = "setBlockGasLimit")]
    async fn anvil_set_block_gas_limit(&self, gas_limit: U256) -> RpcResult<bool>;

    #[method(name = "setBlockTimestampInterval")]
    async fn anvil_set_block_timestamp_interval(&self, seconds: u64) -> RpcResult<()>;

    #[method(name = "removeBlockTimestampInterval")]
    async fn anvil_remove_block_timestamp_interval(&self) -> RpcResult<bool>;

    #[method(name = "mine_detailed")] // This method uses snake_case.
    async fn anvil_mine_detailed(&self, opts: Option<Params<Option<EvmMineOptions>>>) -> RpcResult<Vec<Block>>;

    #[method(name = "enableTraces")]
    async fn anvil_enable_traces(&self) -> RpcResult<()>;

    #[method(name = "removePoolTransactions")]
    async fn anvil_remove_pool_transactions(&self, address: Address) -> RpcResult<()>;
}
