//! Loads a pending block from database. Helper trait for `eth_` block, transaction, call and trace
//! RPC methods.

use futures::Future;
use reth_evm::ConfigureEvmEnv;
use reth_primitives::{Address, BlockId, Bytes, Header, B256, U256};
use reth_provider::{BlockIdReader, StateProvider, StateProviderBox, StateProviderFactory};
use reth_rpc_eth_types::{
    EthApiError, EthResult, EthStateCache, PendingBlockEnv, RpcInvalidTransactionError,
};
use reth_rpc_types::{serde_helpers::JsonStorageKey, EIP1186AccountProofResponse};
use reth_rpc_types_compat::proof::from_primitive_account_proof;
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use revm::db::BundleState;
use revm_primitives::{BlockEnv, CfgEnvWithHandlerCfg, SpecId};

use super::{EthApiSpec, LoadPendingBlock, SpawnBlocking};

/// Helper methods for `eth_` methods relating to state (accounts).
pub trait EthState: LoadState + SpawnBlocking {
    /// Returns the maximum number of blocks into the past for generating state proofs.
    fn max_proof_window(&self) -> u64;

    /// Returns the number of transactions sent from an address at the given block identifier.
    ///
    /// If this is [`BlockNumberOrTag::Pending`](reth_primitives::BlockNumberOrTag) then this will
    /// look up the highest transaction in pool and return the next nonce (highest + 1).
    fn transaction_count(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> impl Future<Output = EthResult<U256>> + Send {
        LoadState::transaction_count(self, address, block_id)
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

        // Check whether the distance to the block exceeds the maximum configured window.
        let block_number = self
            .provider()
            .block_number_for_id(block_id)?
            .ok_or(EthApiError::UnknownBlockNumber)?;
        let max_window = self.max_proof_window();
        if chain_info.best_number.saturating_sub(block_number) > max_window {
            return Err(EthApiError::ExceedsMaxProofWindow)
        }

        Ok(self.spawn_blocking_io(move |this| {
            let state = this.state_at_block_id(block_id)?;
            let storage_keys = keys.iter().map(|key| key.0).collect::<Vec<_>>();
            let proof = state.proof(&BundleState::default(), address, &storage_keys)?;
            Ok(from_primitive_account_proof(proof))
        }))
    }
}

/// Loads state from database.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` state RPC methods.
pub trait LoadState {
    /// Returns a handle for reading state from database.
    ///
    /// Data access in default trait method implementations.
    fn provider(&self) -> impl StateProviderFactory;

    /// Returns a handle for reading data from memory.
    ///
    /// Data access in default (L1) trait method implementations.
    fn cache(&self) -> &EthStateCache;

    /// Returns a handle for reading data from transaction pool.
    ///
    /// Data access in default trait method implementations.
    fn pool(&self) -> impl TransactionPool;

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

    /// Returns the revm evm env for the requested [`BlockId`]
    ///
    /// If the [`BlockId`] this will return the [`BlockId`] of the block the env was configured
    /// for.
    /// If the [`BlockId`] is pending, this will return the "Pending" tag, otherwise this returns
    /// the hash of the exact block.
    fn evm_env_at(
        &self,
        at: BlockId,
    ) -> impl Future<Output = EthResult<(CfgEnvWithHandlerCfg, BlockEnv, BlockId)>> + Send
    where
        Self: LoadPendingBlock + SpawnBlocking,
    {
        async move {
            if at.is_pending() {
                let PendingBlockEnv { cfg, block_env, origin } =
                    self.pending_block_env_and_cfg()?;
                Ok((cfg, block_env, origin.state_block_id()))
            } else {
                // Use cached values if there is no pending block
                let block_hash = LoadPendingBlock::provider(self)
                    .block_hash_for_id(at)?
                    .ok_or_else(|| EthApiError::UnknownBlockNumber)?;
                let (cfg, env) = self.cache().get_evm_env(block_hash).await?;
                Ok((cfg, env, block_hash.into()))
            }
        }
    }

    /// Returns the revm evm env for the raw block header
    ///
    /// This is used for tracing raw blocks
    fn evm_env_for_raw_block(
        &self,
        header: &Header,
    ) -> impl Future<Output = EthResult<(CfgEnvWithHandlerCfg, BlockEnv)>> + Send
    where
        Self: LoadPendingBlock + SpawnBlocking,
    {
        async move {
            // get the parent config first
            let (cfg, mut block_env, _) = self.evm_env_at(header.parent_hash.into()).await?;

            let after_merge = cfg.handler_cfg.spec_id >= SpecId::MERGE;
            self.evm_config().fill_block_env(&mut block_env, header, after_merge);

            Ok((cfg, block_env))
        }
    }

    /// Returns the number of transactions sent from an address at the given block identifier.
    ///
    /// If this is [`BlockNumberOrTag::Pending`](reth_primitives::BlockNumberOrTag) then this will
    /// look up the highest transaction in pool and return the next nonce (highest + 1).
    fn transaction_count(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> impl Future<Output = EthResult<U256>> + Send
    where
        Self: SpawnBlocking,
    {
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
}
