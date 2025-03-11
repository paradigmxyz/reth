//! Loads a pending block from database. Helper trait for `eth_` block, transaction, call and trace
//! RPC methods.

use super::SpawnBlocking;
use crate::{types::RpcTypes, EthApiTypes, FromEthApiError, FromEvmError, RpcNodeCore};
use alloy_consensus::{BlockHeader, Transaction};
use alloy_eips::eip4844::MAX_DATA_GAS_PER_BLOCK;
use alloy_rpc_types_eth::BlockNumberOrTag;
use futures::Future;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_errors::{BlockExecutionError, BlockValidationError, RethError};
use reth_evm::{
    execute::{BlockBuilder, BlockBuilderOutcome},
    ConfigureEvm, Evm, SpecFor,
};
use reth_node_api::NodePrimitives;
use reth_primitives_traits::{
    transaction::error::InvalidTransactionError, Receipt, RecoveredBlock, SealedHeader,
};
use reth_provider::{
    BlockReader, BlockReaderIdExt, ChainSpecProvider, ProviderBlock, ProviderError, ProviderHeader,
    ProviderReceipt, ProviderTx, ReceiptProvider, StateProviderFactory,
};
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_rpc_eth_types::{EthApiError, PendingBlock, PendingBlockEnv, PendingBlockEnvOrigin};
use reth_transaction_pool::{
    error::InvalidPoolTransactionError, BestTransactionsAttributes, PoolTransaction,
    TransactionPool,
};
use revm::context_interface::Block;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::debug;

/// Loads a pending block from database.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` blocks RPC methods.
pub trait LoadPendingBlock:
    EthApiTypes<
        NetworkTypes: RpcTypes<
            Header = alloy_rpc_types_eth::Header<ProviderHeader<Self::Provider>>,
        >,
        Error: FromEvmError<Self::Evm>,
    > + RpcNodeCore<
        Provider: BlockReaderIdExt<Receipt: Receipt>
                      + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
                      + StateProviderFactory,
        Pool: TransactionPool<Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>>,
        Evm: ConfigureEvm<
            Primitives: NodePrimitives<
                BlockHeader = ProviderHeader<Self::Provider>,
                SignedTx = ProviderTx<Self::Provider>,
                Receipt = ProviderReceipt<Self::Provider>,
                Block = ProviderBlock<Self::Provider>,
            >,
        >,
    >
{
    /// Returns a handle to the pending block.
    ///
    /// Data access in default (L1) trait method implementations.
    #[expect(clippy::type_complexity)]
    fn pending_block(
        &self,
    ) -> &Mutex<Option<PendingBlock<ProviderBlock<Self::Provider>, ProviderReceipt<Self::Provider>>>>;

    /// Configures the [`PendingBlockEnv`] for the pending block
    ///
    /// If no pending block is available, this will derive it from the `latest` block
    #[expect(clippy::type_complexity)]
    fn pending_block_env_and_cfg(
        &self,
    ) -> Result<
        PendingBlockEnv<
            ProviderBlock<Self::Provider>,
            ProviderReceipt<Self::Provider>,
            SpecFor<Self::Evm>,
        >,
        Self::Error,
    > {
        if let Some(block) =
            self.provider().pending_block_with_senders().map_err(Self::Error::from_eth_err)?
        {
            if let Some(receipts) = self
                .provider()
                .receipts_by_block(block.hash().into())
                .map_err(Self::Error::from_eth_err)?
            {
                // Note: for the PENDING block we assume it is past the known merge block and
                // thus this will not fail when looking up the total
                // difficulty value for the blockenv.
                let evm_env = self.evm_config().evm_env(block.header());

                return Ok(PendingBlockEnv::new(
                    evm_env,
                    PendingBlockEnvOrigin::ActualPending(block, receipts),
                ));
            }
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
    ) -> Result<<Self::Evm as ConfigureEvm>::NextBlockEnvCtx, Self::Error>;

    /// Returns the locally built pending block
    #[expect(clippy::type_complexity)]
    fn local_pending_block(
        &self,
    ) -> impl Future<
        Output = Result<
            Option<(
                RecoveredBlock<<Self::Provider as BlockReader>::Block>,
                Vec<ProviderReceipt<Self::Provider>>,
            )>,
            Self::Error,
        >,
    > + Send
    where
        Self: SpawnBlocking,
    {
        async move {
            let pending = self.pending_block_env_and_cfg()?;
            let parent = match pending.origin {
                PendingBlockEnvOrigin::ActualPending(block, receipts) => {
                    return Ok(Some((block, receipts)));
                }
                PendingBlockEnvOrigin::DerivedFromLatest(parent) => parent,
            };

            // we couldn't find the real pending block, so we need to build it ourselves
            let mut lock = self.pending_block().lock().await;

            let now = Instant::now();

            // check if the block is still good
            if let Some(pending_block) = lock.as_ref() {
                // this is guaranteed to be the `latest` header
                if pending.evm_env.block_env.number == pending_block.block.number() &&
                    parent.hash() == pending_block.block.parent_hash() &&
                    now <= pending_block.expires_at
                {
                    return Ok(Some((pending_block.block.clone(), pending_block.receipts.clone())));
                }
            }

            // no pending block from the CL yet, so we need to build it ourselves via txpool
            let (sealed_block, receipts) = match self
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

            let now = Instant::now();
            *lock = Some(PendingBlock::new(
                now + Duration::from_secs(1),
                sealed_block.clone(),
                receipts.clone(),
            ));

            Ok(Some((sealed_block, receipts)))
        }
    }

    /// Builds a pending block using the configured provider and pool.
    ///
    /// If the origin is the actual pending block, the block is built with withdrawals.
    ///
    /// After Cancun, if the origin is the actual pending block, the block includes the EIP-4788 pre
    /// block contract call using the parent beacon block root received from the CL.
    #[expect(clippy::type_complexity)]
    fn build_block(
        &self,
        parent: &SealedHeader<ProviderHeader<Self::Provider>>,
    ) -> Result<
        (RecoveredBlock<ProviderBlock<Self::Provider>>, Vec<ProviderReceipt<Self::Provider>>),
        Self::Error,
    >
    where
        EthApiError: From<ProviderError>,
    {
        let state_provider = self
            .provider()
            .history_by_block_hash(parent.hash())
            .map_err(Self::Error::from_eth_err)?;
        let state = StateProviderDatabase::new(&state_provider);
        let mut db = State::builder().with_database(state).with_bundle_update().build();

        let mut builder = self
            .evm_config()
            .builder_for_next_block(&mut db, parent, self.next_env_attributes(parent)?)
            .map_err(RethError::other)
            .map_err(Self::Error::from_eth_err)?;

        builder.apply_pre_execution_changes().map_err(Self::Error::from_eth_err)?;

        let block_env = builder.evm_mut().block().clone();

        let mut cumulative_gas_used = 0;
        let mut sum_blob_gas_used = 0;
        let block_gas_limit: u64 = block_env.gas_limit;

        let mut best_txs =
            self.pool().best_transactions_with_attributes(BestTransactionsAttributes::new(
                block_env.basefee,
                block_env.blob_gasprice().map(|gasprice| gasprice as u64),
            ));

        while let Some(pool_tx) = best_txs.next() {
            // ensure we still have capacity for this transaction
            if cumulative_gas_used + pool_tx.gas_limit() > block_gas_limit {
                // we can't fit this transaction into the block, so we need to mark it as invalid
                // which also removes all dependent transaction from the iterator before we can
                // continue
                best_txs.mark_invalid(
                    &pool_tx,
                    InvalidPoolTransactionError::ExceedsGasLimit(
                        pool_tx.gas_limit(),
                        block_gas_limit,
                    ),
                );
                continue
            }

            if pool_tx.origin.is_private() {
                // we don't want to leak any state changes made by private transactions, so we mark
                // them as invalid here which removes all dependent transactions from the iterator
                // before we can continue
                best_txs.mark_invalid(
                    &pool_tx,
                    InvalidPoolTransactionError::Consensus(
                        InvalidTransactionError::TxTypeNotSupported,
                    ),
                );
                continue
            }

            // convert tx to a signed transaction
            let tx = pool_tx.to_consensus();

            // There's only limited amount of blob space available per block, so we need to check if
            // the EIP-4844 can still fit in the block
            if let Some(tx_blob_gas) = tx.blob_gas_used() {
                if sum_blob_gas_used + tx_blob_gas > MAX_DATA_GAS_PER_BLOCK {
                    // we can't fit this _blob_ transaction into the block, so we mark it as
                    // invalid, which removes its dependent transactions from
                    // the iterator. This is similar to the gas limit condition
                    // for regular transactions above.
                    best_txs.mark_invalid(
                        &pool_tx,
                        InvalidPoolTransactionError::ExceedsGasLimit(
                            tx_blob_gas,
                            MAX_DATA_GAS_PER_BLOCK,
                        ),
                    );
                    continue
                }
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
                            InvalidPoolTransactionError::Consensus(
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
                if sum_blob_gas_used == MAX_DATA_GAS_PER_BLOCK {
                    best_txs.skip_blobs();
                }
            }

            // add gas used by the transaction to cumulative gas used, before creating the receipt
            cumulative_gas_used += gas_used;
        }

        let BlockBuilderOutcome { execution_result, block, .. } =
            builder.finish(&state_provider).map_err(Self::Error::from_eth_err)?;

        Ok((block, execution_result.receipts))
    }
}
