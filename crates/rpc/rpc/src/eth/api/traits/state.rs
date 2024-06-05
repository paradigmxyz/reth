//! Loads state from database. Helper trait for `eth_`transaction and state RPC methods.

use futures::Future;
use reth_primitives::{Address, BlockId, BlockNumberOrTag, Bytes, B256, U256};
use reth_provider::{StateProviderBox, StateProviderFactory};
use reth_rpc_types::{serde_helpers::JsonStorageKey, EIP1186AccountProofResponse};
use reth_rpc_types_compat::proof::from_primitive_account_proof;
use reth_transaction_pool::{PoolTransaction, TransactionPool};

use crate::{
    eth::{
        api::SpawnBlocking,
        error::{EthApiError, EthResult, RpcInvalidTransactionError},
    },
    EthApiSpec,
};

/// Helper methods for `eth_` methods relating to state (accounts).
pub trait EthState: LoadState + SpawnBlocking {
    /// Returns a handle for reading data from transaction pool.
    ///
    /// Data access in default trait method implementations.
    fn pool(&self) -> &impl TransactionPool;

    /// Returns the number of transactions sent from an address at the given block identifier.
    ///
    /// If this is [`BlockNumberOrTag::Pending`](reth_primitives::BlockNumberOrTag) then this will
    /// look up the highest transaction in pool and return the next nonce (highest + 1).
    fn transaction_count(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> impl Future<Output = EthResult<U256>> + Send {
        self.spawn_blocking_io(move |this| {
            if block_id == Some(BlockId::pending()) {
                let address_txs = this.pool().get_transactions_by_sender(address);
                if let Some(highest_nonce) =
                    address_txs.iter().map(|item| item.transaction.nonce()).max()
                {
                    let tx_count = highest_nonce
                        .checked_add(1)
                        .ok_or(RpcInvalidTransactionError::NonceMaxValue)?;
                    return Ok(U256::from(tx_count))
                }
            }

            let state = this.state_at_block_id_or_latest(block_id)?;
            Ok(U256::from(state.account_nonce(address)?.unwrap_or_default()))
        })
    }

    /// Returns code of given account, at given blocknumber.
    fn get_code(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> impl Future<Output = EthResult<Bytes>> + Send {
        self.spawn_blocking_io(move |this| {
            Ok(this
                .state_at_block_id_or_latest(block_id)?
                .account_code(address)?
                .unwrap_or_default()
                .original_bytes())
        })
    }

    /// Returns balance of given account, at given blocknumber.
    fn balance(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> impl Future<Output = EthResult<U256>> + Send {
        self.spawn_blocking_io(move |this| {
            Ok(this
                .state_at_block_id_or_latest(block_id)?
                .account_balance(address)?
                .unwrap_or_default())
        })
    }

    /// Returns values stored of given account, at given blocknumber.
    fn storage_at(
        &self,
        address: Address,
        index: JsonStorageKey,
        block_id: Option<BlockId>,
    ) -> impl Future<Output = EthResult<B256>> + Send {
        self.spawn_blocking_io(move |this| {
            Ok(B256::new(
                this.state_at_block_id_or_latest(block_id)?
                    .storage(address, index.0)?
                    .unwrap_or_default()
                    .to_be_bytes(),
            ))
        })
    }

    /// Returns values stored of given account, with Merkle-proof, at given blocknumber.
    fn get_proof(
        &self,
        address: Address,
        keys: Vec<JsonStorageKey>,
        block_id: Option<BlockId>,
    ) -> EthResult<impl Future<Output = EthResult<EIP1186AccountProofResponse>> + Send>
    where
        Self: EthApiSpec,
    {
        let chain_info = self.chain_info()?;
        let block_id = block_id.unwrap_or_default();

        // if we are trying to create a proof for the latest block, but have a BlockId as input
        // that is not BlockNumberOrTag::Latest, then we need to figure out whether or not the
        // BlockId corresponds to the latest block
        let is_latest_block = match block_id {
            BlockId::Number(BlockNumberOrTag::Number(num)) => num == chain_info.best_number,
            BlockId::Hash(hash) => hash == chain_info.best_hash.into(),
            BlockId::Number(BlockNumberOrTag::Latest) => true,
            _ => false,
        };

        // TODO: remove when HistoricalStateProviderRef::proof is implemented
        if !is_latest_block {
            return Err(EthApiError::InvalidBlockRange)
        }

        Ok(self.spawn_tracing(move |this| {
            let state = this.state_at_block_id(block_id)?;
            let storage_keys = keys.iter().map(|key| key.0).collect::<Vec<_>>();
            let proof = state.proof(address, &storage_keys)?;
            Ok(from_primitive_account_proof(proof))
        }))
    }
}

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
