//! Loads a pending block from database. Helper trait for `eth_` block, transaction, call and trace
//! RPC methods.

use super::SpawnBlocking;
use crate::{EthApiTypes, FromEthApiError, FromEvmError, RpcNodeCore};
use alloy_consensus::{BlockHeader, Transaction};
use alloy_eips::eip7840::BlobParams;
use alloy_primitives::{B256, U256};
use alloy_rpc_types_eth::BlockNumberOrTag;
use futures::Future;
use reth_chain_state::{BlockState, ComputedTrieData, ExecutedBlock};
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_errors::{BlockExecutionError, BlockValidationError, ProviderError, RethError};
use reth_evm::{
    execute::{BlockBuilder, BlockBuilderOutcome, ExecutionOutcome},
    ConfigureEvm, Evm, NextBlockEnvAttributes,
};
use reth_primitives_traits::{transaction::error::InvalidTransactionError, HeaderTy, SealedHeader};
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_types::{
    block::BlockAndReceipts, builder::config::PendingBlockKind, EthApiError, PendingBlock,
    PendingBlockEnv, PendingBlockEnvOrigin,
};
use reth_storage_api::{
    noop::NoopProvider, BlockReader, BlockReaderIdExt, ProviderHeader, ProviderTx, ReceiptProvider,
    StateProviderBox, StateProviderFactory,
};
use reth_transaction_pool::{
    error::InvalidPoolTransactionError, BestTransactions, BestTransactionsAttributes,
    PoolTransaction, TransactionPool,
};
use revm::context_interface::Block;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tracing::debug;

/// Loads a pending block from database.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` blocks RPC methods.
pub trait LoadPendingBlock:
    EthApiTypes<
        Error: FromEvmError<Self::Evm>,
        RpcConvert: RpcConvert<Network = Self::NetworkTypes>,
    > + RpcNodeCore
{
    /// Returns a handle to the pending block.
    ///
    /// Data access in default (L1) trait method implementations.
    fn pending_block(&self) -> &Mutex<Option<PendingBlock<Self::Primitives>>>;

    /// Returns a [`PendingEnvBuilder`] for the pending block.
    fn pending_env_builder(&self) -> &dyn PendingEnvBuilder<Self::Evm>;

    /// Returns the pending block kind
    fn pending_block_kind(&self) -> PendingBlockKind;

    /// Configures the [`PendingBlockEnv`] for the pending block
    ///
    /// If no pending block is available, this will derive it from the `latest` block
    fn pending_block_env_and_cfg(&self) -> Result<PendingBlockEnv<Self::Evm>, Self::Error> {
        if let Some(block) = self.provider().pending_block().map_err(Self::Error::from_eth_err)? &&
            let Some(receipts) = self
                .provider()
                .receipts_by_block(block.hash().into())
                .map_err(Self::Error::from_eth_err)?
        {
            // Note: for the PENDING block we assume it is past the known merge block and
            // thus this will not fail when looking up the total
            // difficulty value for the blockenv.
            let evm_env = self
                .evm_config()
                .evm_env(block.header())
                .map_err(RethError::other)
                .map_err(Self::Error::from_eth_err)?;

            return Ok(PendingBlockEnv::new(
                evm_env,
                PendingBlockEnvOrigin::ActualPending(Arc::new(block), Arc::new(receipts)),
            ));
        }

        // no pending block from the CL yet, so we use the latest block and modify the env
        // values that we can
        let latest = self
            .provider()
            .latest_header()
            .map_err(Self::Error::from_eth_err)?
            .ok_or(EthApiError::HeaderNotFound(BlockNumberOrTag::Latest.into()))?;

        let evm_env = self
            .evm_config()
            .next_evm_env(&latest, &self.next_env_attributes(&latest)?)
            .map_err(RethError::other)
            .map_err(Self::Error::from_eth_err)?;

        Ok(PendingBlockEnv::new(evm_env, PendingBlockEnvOrigin::DerivedFromLatest(latest)))
    }

    /// Returns [`ConfigureEvm::NextBlockEnvCtx`] for building a local pending block.
    fn next_env_attributes(
        &self,
        parent: &SealedHeader<ProviderHeader<Self::Provider>>,
    ) -> Result<<Self::Evm as ConfigureEvm>::NextBlockEnvCtx, Self::Error> {
        Ok(self.pending_env_builder().pending_env_attributes(parent)?)
    }

    /// Returns a [`StateProviderBox`] on a mem-pool built pending block overlaying latest.
    fn local_pending_state(
        &self,
    ) -> impl Future<Output = Result<Option<StateProviderBox>, Self::Error>> + Send
    where
        Self: SpawnBlocking,
    {
        async move {
            let Some(pending_block) = self.pool_pending_block().await? else {
                return Ok(None);
            };

            let latest_historical = self
                .provider()
                .history_by_block_hash(pending_block.block().parent_hash())
                .map_err(Self::Error::from_eth_err)?;

            let state = BlockState::from(pending_block);

            Ok(Some(Box::new(state.state_provider(latest_historical)) as StateProviderBox))
        }
    }

    /// Returns a mem-pool built pending block.
    fn pool_pending_block(
        &self,
    ) -> impl Future<Output = Result<Option<PendingBlock<Self::Primitives>>, Self::Error>> + Send
    where
        Self: SpawnBlocking,
    {
        async move {
            if self.pending_block_kind().is_none() {
                return Ok(None);
            }
            let pending = self.pending_block_env_and_cfg()?;
            let parent = match pending.origin {
                PendingBlockEnvOrigin::ActualPending(..) => return Ok(None),
                PendingBlockEnvOrigin::DerivedFromLatest(parent) => parent,
            };

            // we couldn't find the real pending block, so we need to build it ourselves
            let mut lock = self.pending_block().lock().await;

            let now = Instant::now();

            // Is the pending block cached?
            if let Some(pending_block) = lock.as_ref() {
                // Is the cached block not expired and latest is its parent?
                if pending.evm_env.block_env.number() == U256::from(pending_block.block().number()) &&
                    parent.hash() == pending_block.block().parent_hash() &&
                    now <= pending_block.expires_at
                {
                    return Ok(Some(pending_block.clone()));
                }
            }

            let executed_block = match self
                .spawn_blocking_io(move |this| {
                    // we rebuild the block
                    this.build_block(&parent)
                })
                .await
            {
                Ok(block) => block,
                Err(err) => {
                    debug!(target: "rpc", "Failed to build pending block: {:?}", err);
                    return Ok(None)
                }
            };

            let pending = PendingBlock::with_executed_block(
                Instant::now() + Duration::from_secs(1),
                executed_block,
            );

            *lock = Some(pending.clone());

            Ok(Some(pending))
        }
    }

    /// Returns the locally built pending block
    fn local_pending_block(
        &self,
    ) -> impl Future<Output = Result<Option<BlockAndReceipts<Self::Primitives>>, Self::Error>> + Send
    where
        Self: SpawnBlocking,
        Self::Pool:
            TransactionPool<Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>>,
    {
        async move {
            if self.pending_block_kind().is_none() {
                return Ok(None);
            }

            let pending = self.pending_block_env_and_cfg()?;

            Ok(match pending.origin {
                PendingBlockEnvOrigin::ActualPending(block, receipts) => {
                    Some(BlockAndReceipts { block, receipts })
                }
                PendingBlockEnvOrigin::DerivedFromLatest(..) => {
                    self.pool_pending_block().await?.map(PendingBlock::into_block_and_receipts)
                }
            })
        }
    }

    /// Builds a locally derived pending block using the configured provider and pool.
    ///
    /// This is used when no execution-layer pending block is available and a pending block is
    /// derived from the latest canonical header, using the provided parent.
    ///
    /// Withdrawals and any fork-specific behavior (such as EIP-4788 pre-block contract calls) are
    /// determined by the EVM environment and chain specification used during construction.
    fn build_block(
        &self,
        parent: &SealedHeader<ProviderHeader<Self::Provider>>,
    ) -> Result<ExecutedBlock<Self::Primitives>, Self::Error>
    where
        Self::Pool:
            TransactionPool<Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>>,
        EthApiError: From<ProviderError>,
    {
        let state_provider = self
            .provider()
            .history_by_block_hash(parent.hash())
            .map_err(Self::Error::from_eth_err)?;
        let state = StateProviderDatabase::new(state_provider);
        let mut db = State::builder().with_database(state).with_bundle_update().build();

        let mut builder = self
            .evm_config()
            .builder_for_next_block(&mut db, parent, self.next_env_attributes(parent)?)
            .map_err(RethError::other)
            .map_err(Self::Error::from_eth_err)?;

        builder.apply_pre_execution_changes().map_err(Self::Error::from_eth_err)?;

        let block_env = builder.evm_mut().block().clone();

        let blob_params = self
            .provider()
            .chain_spec()
            .blob_params_at_timestamp(parent.timestamp())
            .unwrap_or_else(BlobParams::cancun);
        let mut cumulative_gas_used = 0;
        let mut sum_blob_gas_used = 0;
        let block_gas_limit: u64 = block_env.gas_limit();

        // Only include transactions if not configured as Empty
        if !self.pending_block_kind().is_empty() {
            let mut best_txs = self
                .pool()
                .best_transactions_with_attributes(BestTransactionsAttributes::new(
                    block_env.basefee(),
                    block_env.blob_gasprice().map(|gasprice| gasprice as u64),
                ))
                // freeze to get a block as fast as possible
                .without_updates();

            while let Some(pool_tx) = best_txs.next() {
                // ensure we still have capacity for this transaction
                if cumulative_gas_used + pool_tx.gas_limit() > block_gas_limit {
                    // we can't fit this transaction into the block, so we need to mark it as
                    // invalid which also removes all dependent transaction from
                    // the iterator before we can continue
                    best_txs.mark_invalid(
                        &pool_tx,
                        &InvalidPoolTransactionError::ExceedsGasLimit(
                            pool_tx.gas_limit(),
                            block_gas_limit,
                        ),
                    );
                    continue
                }

                if pool_tx.origin.is_private() {
                    // we don't want to leak any state changes made by private transactions, so we
                    // mark them as invalid here which removes all dependent
                    // transactions from the iteratorbefore we can continue
                    best_txs.mark_invalid(
                        &pool_tx,
                        &InvalidPoolTransactionError::Consensus(
                            InvalidTransactionError::TxTypeNotSupported,
                        ),
                    );
                    continue
                }

                // convert tx to a signed transaction
                let tx = pool_tx.to_consensus();

                // There's only limited amount of blob space available per block, so we need to
                // check if the EIP-4844 can still fit in the block
                if let Some(tx_blob_gas) = tx.blob_gas_used() &&
                    sum_blob_gas_used + tx_blob_gas > blob_params.max_blob_gas_per_block()
                {
                    // we can't fit this _blob_ transaction into the block, so we mark it as
                    // invalid, which removes its dependent transactions from
                    // the iterator. This is similar to the gas limit condition
                    // for regular transactions above.
                    best_txs.mark_invalid(
                        &pool_tx,
                        &InvalidPoolTransactionError::ExceedsGasLimit(
                            tx_blob_gas,
                            blob_params.max_blob_gas_per_block(),
                        ),
                    );
                    continue
                }

                let gas_used = match builder.execute_transaction(tx.clone()) {
                    Ok(gas_used) => gas_used,
                    Err(BlockExecutionError::Validation(BlockValidationError::InvalidTx {
                        error,
                        ..
                    })) => {
                        if error.is_nonce_too_low() {
                            // if the nonce is too low, we can skip this transaction
                        } else {
                            // if the transaction is invalid, we can skip it and all of its
                            // descendants
                            best_txs.mark_invalid(
                                &pool_tx,
                                &InvalidPoolTransactionError::Consensus(
                                    InvalidTransactionError::TxTypeNotSupported,
                                ),
                            );
                        }
                        continue
                    }
                    // this is an error that we should treat as fatal for this attempt
                    Err(err) => return Err(Self::Error::from_eth_err(err)),
                };

                // add to the total blob gas used if the transaction successfully executed
                if let Some(tx_blob_gas) = tx.blob_gas_used() {
                    sum_blob_gas_used += tx_blob_gas;

                    // if we've reached the max data gas per block, we can skip blob txs entirely
                    if sum_blob_gas_used == blob_params.max_blob_gas_per_block() {
                        best_txs.skip_blobs();
                    }
                }

                // add gas used by the transaction to cumulative gas used, before creating the
                // receipt
                cumulative_gas_used += gas_used;
            }
        }

        let BlockBuilderOutcome { execution_result, block, hashed_state, trie_updates, .. } =
            builder.finish(NoopProvider::default()).map_err(Self::Error::from_eth_err)?;

        let execution_outcome = ExecutionOutcome::new(
            db.take_bundle(),
            vec![execution_result.receipts],
            block.number(),
            vec![execution_result.requests],
        );

        Ok(ExecutedBlock::new(
            block.into(),
            Arc::new(execution_outcome),
            ComputedTrieData::without_trie_input(
                Arc::new(hashed_state.into_sorted()),
                Arc::new(trie_updates.into_sorted()),
            ),
        ))
    }
}

/// A type that knows how to build a [`ConfigureEvm::NextBlockEnvCtx`] for a pending block.
pub trait PendingEnvBuilder<Evm: ConfigureEvm>: Send + Sync + Unpin + 'static {
    /// Builds a [`ConfigureEvm::NextBlockEnvCtx`] for pending block.
    fn pending_env_attributes(
        &self,
        parent: &SealedHeader<HeaderTy<Evm::Primitives>>,
    ) -> Result<Evm::NextBlockEnvCtx, EthApiError>;
}

/// Trait that should be implemented on [`ConfigureEvm::NextBlockEnvCtx`] to provide a way for it to
/// build an environment for pending block.
///
/// This assumes that next environment building doesn't require any additional context, for more
/// complex implementations one should implement [`PendingEnvBuilder`] on their custom type.
pub trait BuildPendingEnv<Header> {
    /// Builds a [`ConfigureEvm::NextBlockEnvCtx`] for pending block.
    fn build_pending_env(parent: &SealedHeader<Header>) -> Self;
}

impl<Evm> PendingEnvBuilder<Evm> for ()
where
    Evm: ConfigureEvm<NextBlockEnvCtx: BuildPendingEnv<HeaderTy<Evm::Primitives>>>,
{
    fn pending_env_attributes(
        &self,
        parent: &SealedHeader<HeaderTy<Evm::Primitives>>,
    ) -> Result<Evm::NextBlockEnvCtx, EthApiError> {
        Ok(Evm::NextBlockEnvCtx::build_pending_env(parent))
    }
}

impl<H: BlockHeader> BuildPendingEnv<H> for NextBlockEnvAttributes {
    fn build_pending_env(parent: &SealedHeader<H>) -> Self {
        Self {
            timestamp: parent.timestamp().saturating_add(12),
            suggested_fee_recipient: parent.beneficiary(),
            prev_randao: B256::random(),
            gas_limit: parent.gas_limit(),
            parent_beacon_block_root: parent.parent_beacon_block_root(),
            withdrawals: parent.withdrawals_root().map(|_| Default::default()),
            extra_data: parent.extra_data().clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::B256;
    use reth_primitives_traits::SealedHeader;

    #[test]
    fn pending_env_keeps_parent_beacon_root() {
        let mut header = Header::default();
        let beacon_root = B256::repeat_byte(0x42);
        header.parent_beacon_block_root = Some(beacon_root);
        let sealed = SealedHeader::new(header, B256::ZERO);

        let attrs = NextBlockEnvAttributes::build_pending_env(&sealed);

        assert_eq!(attrs.parent_beacon_block_root, Some(beacon_root));
    }
}
