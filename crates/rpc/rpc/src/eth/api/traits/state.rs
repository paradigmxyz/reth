//! Loads state from database. Helper trait for `eth_`transaction and state RPC methods.

use reth_primitives::{BlockId, B256};
use reth_provider::{StateProviderBox, StateProviderFactory};

use crate::eth::error::EthResult;

/// Loads state from database.
pub trait LoadState {
    /// Returns a handle for reading state from database.
    ///
    /// Data access in default trait method implementations.
    fn provider(&self) -> &impl StateProviderFactory;

    /// Returns the state at the given block number
    fn state_at_hash(&self, block_hash: B256) -> EthResult<StateProviderBox> {
        Ok(self.provider().history_by_block_hash(block_hash)?)
    }

    /// Returns the state at the given [`BlockId`] enum.
    ///
    /// Note: if not [`BlockNumberOrTag::Pending`](reth_primitives::BlockNumberOrTag) then this
    /// will only return canonical state. See also <https://github.com/paradigmxyz/reth/issues/4515>
    fn state_at_block_id(&self, at: BlockId) -> EthResult<StateProviderBox> {
        Ok(self.provider().state_by_block_id(at)?)
    }

    /// Returns the _latest_ state
    fn latest_state(&self) -> EthResult<StateProviderBox> {
        Ok(self.provider().latest()?)
    }

    /// Returns the state at the given [`BlockId`] enum or the latest.
    ///
    /// Convenience function to interprets `None` as `BlockId::Number(BlockNumberOrTag::Latest)`
    fn state_at_block_id_or_latest(
        &self,
        block_id: Option<BlockId>,
    ) -> EthResult<StateProviderBox> {
        if let Some(block_id) = block_id {
            self.state_at_block_id(block_id)
        } else {
            Ok(self.latest_state()?)
        }
    }
}
