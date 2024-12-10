//! Loads a pending block from database. Helper trait for `eth_` block, transaction, call and trace
//! RPC methods.

use super::SpawnBlocking;
use crate::{EthApiTypes, FromEthApiError, FromEvmError, RpcNodeCore};
use alloy_consensus::{BlockHeader, Transaction};
use alloy_eips::eip4844::MAX_DATA_GAS_PER_BLOCK;
use alloy_network::Network;
use alloy_primitives::B256;
use alloy_rpc_types_eth::BlockNumberOrTag;
use futures::Future;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_errors::RethError;
use reth_evm::{
    state_change::post_block_withdrawals_balance_increments, system_calls::SystemCaller,
    ConfigureEvm, ConfigureEvmEnv, NextBlockEnvAttributes,
};
use reth_execution_types::ExecutionOutcome;
use reth_primitives::{BlockExt, InvalidTransactionError, RecoveredTx, SealedBlockWithSenders};
use reth_primitives_traits::receipt::ReceiptExt;
use reth_provider::{
    BlockReader, BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, ProviderBlock, ProviderError,
    ProviderHeader, ProviderReceipt, ProviderTx, ReceiptProvider, StateProviderFactory,
};
use reth_revm::{
    database::StateProviderDatabase,
    primitives::{
        BlockEnv, CfgEnvWithHandlerCfg, EVMError, Env, ExecutionResult, InvalidTransaction,
        ResultAndState,
    },
};
use reth_rpc_eth_types::{EthApiError, PendingBlock, PendingBlockEnv, PendingBlockEnvOrigin};
use reth_transaction_pool::{
    error::InvalidPoolTransactionError, BestTransactionsAttributes, PoolTransaction,
    TransactionPool,
};
use revm::{db::states::bundle_state::BundleRetention, DatabaseCommit, State};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::debug;

/// Loads a pending block from database.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` blocks RPC methods.
pub trait LoadPendingBlock:
    EthApiTypes<
        NetworkTypes: Network<
            HeaderResponse = alloy_rpc_types_eth::Header<ProviderHeader<Self::Provider>>,
        >,
    > + RpcNodeCore<
        Provider: BlockReaderIdExt<Receipt: ReceiptExt>
                      + EvmEnvProvider<ProviderHeader<Self::Provider>>
                      + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
                      + StateProviderFactory,
        Pool: TransactionPool<Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>>,
        Evm: ConfigureEvm<
            Header = ProviderHeader<Self::Provider>,
            Transaction = ProviderTx<Self::Provider>,
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

    /// Configures the [`CfgEnvWithHandlerCfg`] and [`BlockEnv`] for the pending block
    ///
    /// If no pending block is available, this will derive it from the `latest` block
    #[expect(clippy::type_complexity)]
    fn pending_block_env_and_cfg(
        &self,
    ) -> Result<
        PendingBlockEnv<ProviderBlock<Self::Provider>, ProviderReceipt<Self::Provider>>,
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
                let (cfg, block_env) = self
                    .provider()
                    .env_with_header(block.header(), self.evm_config().clone())
                    .map_err(Self::Error::from_eth_err)?;

                return Ok(PendingBlockEnv::new(
                    cfg,
                    block_env,
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

        let (cfg, block_env) = self
            .evm_config()
            .next_cfg_and_block_env(
                &latest,
                NextBlockEnvAttributes {
                    timestamp: latest.timestamp() + 12,
                    suggested_fee_recipient: latest.beneficiary(),
                    prev_randao: B256::random(),
                },
            )
            .map_err(RethError::other)
            .map_err(Self::Error::from_eth_err)?;

        Ok(PendingBlockEnv::new(
            cfg,
            block_env,
            PendingBlockEnvOrigin::DerivedFromLatest(latest.hash()),
        ))
    }

    /// Returns the locally built pending block
    #[expect(clippy::type_complexity)]
    fn local_pending_block(
        &self,
    ) -> impl Future<
        Output = Result<
            Option<(
                SealedBlockWithSenders<<Self::Provider as BlockReader>::Block>,
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
            let parent_hash = match pending.origin {
                PendingBlockEnvOrigin::ActualPending(block, receipts) => {
                    return Ok(Some((block, receipts)));
                }
                PendingBlockEnvOrigin::DerivedFromLatest(parent_hash) => parent_hash,
            };

            // we couldn't find the real pending block, so we need to build it ourselves
            let mut lock = self.pending_block().lock().await;

            let now = Instant::now();

            // check if the block is still good
            if let Some(pending_block) = lock.as_ref() {
                // this is guaranteed to be the `latest` header
                if pending.block_env.number.to::<u64>() == pending_block.block.number() &&
                    parent_hash == pending_block.block.parent_hash() &&
                    now <= pending_block.expires_at
                {
                    return Ok(Some((pending_block.block.clone(), pending_block.receipts.clone())));
                }
            }

            // no pending block from the CL yet, so we need to build it ourselves via txpool
            let (sealed_block, receipts) = match self
                .spawn_blocking_io(move |this| {
                    // we rebuild the block
                    this.build_block(pending.cfg, pending.block_env, parent_hash)
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

    /// Assembles a receipt for a transaction, based on its [`ExecutionResult`].
    fn assemble_receipt(
        &self,
        tx: &RecoveredTx<ProviderTx<Self::Provider>>,
        result: ExecutionResult,
        cumulative_gas_used: u64,
    ) -> ProviderReceipt<Self::Provider>;

    /// Assembles a pending block.
    fn assemble_block(
        &self,
        cfg: CfgEnvWithHandlerCfg,
        block_env: BlockEnv,
        parent_hash: revm_primitives::B256,
        state_root: revm_primitives::B256,
        transactions: Vec<ProviderTx<Self::Provider>>,
        receipts: &[ProviderReceipt<Self::Provider>],
    ) -> ProviderBlock<Self::Provider>;

    /// Builds a pending block using the configured provider and pool.
    ///
    /// If the origin is the actual pending block, the block is built with withdrawals.
    ///
    /// After Cancun, if the origin is the actual pending block, the block includes the EIP-4788 pre
    /// block contract call using the parent beacon block root received from the CL.
    #[expect(clippy::type_complexity)]
    fn build_block(
        &self,
        cfg: CfgEnvWithHandlerCfg,
        block_env: BlockEnv,
        parent_hash: B256,
    ) -> Result<
        (
            SealedBlockWithSenders<ProviderBlock<Self::Provider>>,
            Vec<ProviderReceipt<Self::Provider>>,
        ),
        Self::Error,
    >
    where
        EthApiError: From<ProviderError>,
    {
        let state_provider = self
            .provider()
            .history_by_block_hash(parent_hash)
            .map_err(Self::Error::from_eth_err)?;
        let state = StateProviderDatabase::new(state_provider);
        let mut db = State::builder().with_database(state).with_bundle_update().build();

        let mut cumulative_gas_used = 0;
        let mut sum_blob_gas_used = 0;
        let block_gas_limit: u64 = block_env.gas_limit.to::<u64>();
        let base_fee = block_env.basefee.to::<u64>();
        let block_number = block_env.number.to::<u64>();

        let mut executed_txs = Vec::new();
        let mut senders = Vec::new();
        let mut best_txs =
            self.pool().best_transactions_with_attributes(BestTransactionsAttributes::new(
                base_fee,
                block_env.get_blob_gasprice().map(|gasprice| gasprice as u64),
            ));

        let chain_spec = self.provider().chain_spec();

        let mut system_caller = SystemCaller::new(self.evm_config().clone(), chain_spec.clone());

        system_caller
            .pre_block_blockhashes_contract_call(&mut db, &cfg, &block_env, parent_hash)
            .map_err(|err| EthApiError::Internal(err.into()))?;

        let mut receipts = Vec::new();

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

            // Configure the environment for the block.
            let env = Env::boxed(
                cfg.cfg_env.clone(),
                block_env.clone(),
                Self::evm_config(self).tx_env(tx.as_signed(), tx.signer()),
            );

            let mut evm = revm::Evm::builder().with_env(env).with_db(&mut db).build();

            let ResultAndState { result, state } = match evm.transact() {
                Ok(res) => res,
                Err(err) => {
                    match err {
                        EVMError::Transaction(err) => {
                            if matches!(err, InvalidTransaction::NonceTooLow { .. }) {
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
                        err => {
                            // this is an error that we should treat as fatal for this attempt
                            return Err(Self::Error::from_evm_err(err))
                        }
                    }
                }
            };
            // drop evm to release db reference.
            drop(evm);
            // commit changes
            db.commit(state);

            // add to the total blob gas used if the transaction successfully executed
            if let Some(tx_blob_gas) = tx.blob_gas_used() {
                sum_blob_gas_used += tx_blob_gas;

                // if we've reached the max data gas per block, we can skip blob txs entirely
                if sum_blob_gas_used == MAX_DATA_GAS_PER_BLOCK {
                    best_txs.skip_blobs();
                }
            }

            let gas_used = result.gas_used();

            // add gas used by the transaction to cumulative gas used, before creating the receipt
            cumulative_gas_used += gas_used;

            // Push transaction changeset and calculate header bloom filter for receipt.
            receipts.push(Some(self.assemble_receipt(&tx, result, cumulative_gas_used)));

            // append transaction to the list of executed transactions
            let (tx, sender) = tx.to_components();
            executed_txs.push(tx);
            senders.push(sender);
        }

        // executes the withdrawals and commits them to the Database and BundleState.
        let balance_increments = post_block_withdrawals_balance_increments(
            chain_spec.as_ref(),
            block_env.timestamp.try_into().unwrap_or(u64::MAX),
            &[],
        );

        // increment account balances for withdrawals
        db.increment_balances(balance_increments).map_err(Self::Error::from_eth_err)?;

        // merge all transitions into bundle state.
        db.merge_transitions(BundleRetention::PlainState);

        let execution_outcome: ExecutionOutcome<ProviderReceipt<Self::Provider>> =
            ExecutionOutcome::new(
                db.take_bundle(),
                vec![receipts.clone()].into(),
                block_number,
                Vec::new(),
            );
        let hashed_state = db.database.hashed_post_state(execution_outcome.state());

        // calculate the state root
        let state_root = db.database.state_root(hashed_state).map_err(Self::Error::from_eth_err)?;

        // Convert Vec<Option<Receipt>> to Vec<Receipt>
        let receipts: Vec<_> = receipts.into_iter().flatten().collect();
        let block =
            self.assemble_block(cfg, block_env, parent_hash, state_root, executed_txs, &receipts);

        Ok((SealedBlockWithSenders { block: block.seal_slow(), senders }, receipts))
    }
}
