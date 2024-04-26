use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_primitives::U256;
use reth_rpc_types::anvil::MineOptions;

/// Ganache rpc interface.
/// https://github.com/trufflesuite/ganache/tree/develop/docs
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "evm"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "evm"))]
pub trait GanacheApi {
    // TODO Ganache is deprecated and this method is not implemented by Anvil and Hardhat.
    // #[method(name = "addAccount")]
    // async fn evm_add_account(&self, address: Address, passphrase: B256) -> RpcResult<bool>;

    /// Jump forward in time by the given amount of time, in seconds.
    ///
    /// Returns the total time adjustment, in seconds.
    #[method(name = "increaseTime")]
    async fn evm_increase_time(&self, seconds: U256) -> RpcResult<i64>;

    /// Force a single block to be mined.
    ///
    /// Mines a block independent of whether or not mining is started or stopped. Will mine an empty
    /// block if there are no available transactions to mine.
    ///
    /// Returns "0x0". May return additional meta-data in the future.
    #[method(name = "mine")]
    async fn evm_mine(&self, opts: Option<MineOptions>) -> RpcResult<String>;

    // TODO Ganache is deprecated and this method is not implemented by Anvil and Hardhat.
    // #[method(name = "removeAccount")]
    // async fn evm_remove_account(address: Address, passphrase: B256) -> RpcResult<bool>;

    /// Revert the state of the blockchain to a previous snapshot. Takes a single parameter, which
    /// is the snapshot id to revert to. This deletes the given snapshot, as well as any snapshots
    /// taken after (e.g.: reverting to id 0x1 will delete snapshots with ids 0x1, 0x2, etc.).
    ///
    /// Returns `true` if a snapshot was reverted, otherwise `false`.
    #[method(name = "revert")]
    async fn evm_revert(&self, snapshot_id: U256) -> RpcResult<bool>;

    // TODO Ganache is deprecated and this method is not implemented by Anvil and Hardhat.
    // #[method(name = "setAccountBalance")]
    // async fn evm_set_account_balance(address: Address, balance: U256) -> RpcResult<bool>;

    // TODO Ganache is deprecated and this method is not implemented by Anvil and Hardhat.
    // #[method(name = "setAccountCode")]
    // async fn evm_set_account_code(address: Address, code: Bytes) -> RpcResult<bool>;

    // TODO Ganache is deprecated and this method is not implemented by Anvil and Hardhat.
    // #[method(name = "setAccountNonce")]
    // async fn evm_set_account_nonce(address: Address, nonce: U256) -> RpcResult<bool>;

    // TODO Ganache is deprecated and this method is not implemented by Anvil and Hardhat.
    // #[method(name = "setAccountStorageAt")]
    // async fn evm_set_account_storage_at(address: Address, slot: U256, value: B256) ->
    // RpcResult<bool>;

    /// Sets the internal clock time to the given timestamp.
    ///
    /// **Warning** This will allow you to move backwards in time, which may cause new blocks to
    /// appear to be mined before old blocks. This will result in an invalid state.
    ///
    /// Returns the amount of seconds between the given timestamp and now.
    #[method(name = "setTime")]
    async fn evm_set_time(&self, timestamp: u64) -> RpcResult<bool>;

    /// Snapshot the state of the blockchain at the current block. Takes no parameters. Returns the
    /// id of the snapshot that was created. A snapshot can only be reverted once. After a
    /// successful evm_revert, the same snapshot id cannot be used again. Consider creating a new
    /// snapshot after each evm_revert if you need to revert to the same point multiple times.
    ///
    /// Returns the hex-encoded identifier for this snapshot.
    #[method(name = "snapshot")]
    async fn evm_snapshot(&self) -> RpcResult<U256>;
}
