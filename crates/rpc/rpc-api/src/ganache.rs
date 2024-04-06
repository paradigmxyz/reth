use jsonrpsee::{core::RpcResult, proc_macros::rpc};

use reth_primitives::U256;
use reth_rpc_types::ganache::MineOptions;

/// Ganache rpc interface.
/// https://github.com/trufflesuite/ganache/tree/develop/docs
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "evm"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "evm"))]
pub trait GanacheApi {
    // #[method(name = "addAccount")]
    // async fn evm_add_account(&self, address: Address, passphrase: B256) -> RpcResult<bool>;

    #[method(name = "increaseTime")]
    async fn evm_increase_time(&self, seconds: U256) -> RpcResult<i64>;

    #[method(name = "mine")]
    async fn evm_mine(&self, opts: Option<MineOptions>) -> RpcResult<String>;

    // #[method(name = "removeAccount")]
    // async fn evm_remove_account(address: Address, passphrase: B256) -> RpcResult<bool>;

    #[method(name = "revert")]
    async fn evm_revert(&self, snapshot_id: U256) -> RpcResult<bool>;

    // #[method(name = "setAccountBalance")]
    // async fn evm_set_account_balance(address: Address, balance: U256) -> RpcResult<bool>;

    // #[method(name = "setAccountCode")]
    // async fn evm_set_account_code(address: Address, code: Bytes) -> RpcResult<bool>;

    // #[method(name = "setAccountNonce")]
    // async fn evm_set_account_nonce(address: Address, nonce: U256) -> RpcResult<bool>;

    // #[method(name = "setAccountStorageAt")]
    // async fn evm_set_account_storage_at(address: Address, slot: U256, value: B256) -> RpcResult<bool>;

    #[method(name = "setTime")]
    async fn evm_set_time(&self, timestamp: u64) -> RpcResult<bool>;

    #[method(name = "snapshot")]
    async fn evm_snapshot(&self) -> RpcResult<U256>;
}
