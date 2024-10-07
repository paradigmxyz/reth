//! Loads a pending block from database. Helper trait for `eth_` block, transaction, call and trace
//! RPC methods.

use std::time::{Duration, Instant};

use crate::{EthApiTypes, FromEthApiError, FromEvmError};

use alloy_primitives::{BlockNumber, B256, U256};
use alloy_rpc_types::BlockNumberOrTag;
use futures::Future;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_evm::{system_calls::SystemCaller, ConfigureEvm, ConfigureEvmEnv};
use reth_execution_types::ExecutionOutcome;
use reth_primitives::{
    constants::{eip4844::MAX_DATA_GAS_PER_BLOCK, BEACON_NONCE, EMPTY_ROOT_HASH},
    proofs::calculate_transaction_root,
    revm_primitives::{
        BlockEnv, CfgEnv, CfgEnvWithHandlerCfg, EVMError, Env, ExecutionResult, InvalidTransaction,
        ResultAndState, SpecId,
    },
    Block, BlockBody, Header, Receipt, Requests, SealedBlockWithSenders, SealedHeader,
    TransactionSignedEcRecovered, EMPTY_OMMER_ROOT_HASH,
};
use reth_provider::{
    BlockReader, BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, ProviderError,
    ReceiptProvider, StateProviderFactory,
};
use reth_revm::{
    database::StateProviderDatabase, state_change::post_block_withdrawals_balance_increments,
};
use reth_rpc_eth_types::{EthApiError, PendingBlock, PendingBlockEnv, PendingBlockEnvOrigin};
use reth_transaction_pool::{BestTransactionsAttributes, TransactionPool};
use reth_trie::HashedPostState;
use revm::{db::states::bundle_state::BundleRetention, DatabaseCommit, State};
use tokio::sync::Mutex;
use tracing::debug;

use super::SpawnBlocking;

/// Loads a pending block from database.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` blocks RPC methods.
pub trait LoadPendingBlock: EthApiTypes {
    /// Returns a handle for reading data from disk.
    ///
    /// Data access in default (L1) trait method implementations.
    fn provider(
        &self,
    ) -> impl BlockReaderIdExt
           + EvmEnvProvider
           + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
           + StateProviderFactory;

    /// Returns a handle for reading data from transaction pool.
    ///
    /// Data access in default (L1) trait method implementations.
    fn pool(&self) -> impl TransactionPool;

    /// Returns a handle to the pending block.
    ///
    /// Data access in default (L1) trait method implementations.
    fn pending_block(&self) -> &Mutex<Option<PendingBlock>>;

    /// Returns a handle for reading evm config.
    ///
    /// Data access in default (L1) trait method implementations.
    fn evm_config(&self) -> &impl ConfigureEvm<Header = Header>;

    /// Configures the [`CfgEnvWithHandlerCfg`] and [`BlockEnv`] for the pending block
    ///
    /// If no pending block is available, this will derive it from the `latest` block
    fn pending_block_env_and_cfg(&self) -> Result<PendingBlockEnv, Self::Error> {
        let origin: PendingBlockEnvOrigin = if let Some(pending) =
            self.provider().pending_block_with_senders().map_err(Self::Error::from_eth_err)?
        {
            PendingBlockEnvOrigin::ActualPending(pending)
        } else {
            // no pending block from the CL yet, so we use the latest block and modify the env
            // values that we can
            let latest = self
                .provider()
                .latest_header()
                .map_err(Self::Error::from_eth_err)?
                .ok_or(EthApiError::HeaderNotFound(BlockNumberOrTag::Latest.into()))?;

            let (mut latest_header, block_hash) = latest.split();
            // child block
            latest_header.number += 1;
            // assumed child block is in the next slot: 12s
            latest_header.timestamp += 12;
            // base fee of the child block
            let chain_spec = self.provider().chain_spec();

            latest_header.base_fee_per_gas = latest_header.next_block_base_fee(
                chain_spec.base_fee_params_at_timestamp(latest_header.timestamp),
            );

            // update excess blob gas consumed above target
            latest_header.excess_blob_gas = latest_header.next_block_excess_blob_gas();

            // we're reusing the same block hash because we need this to lookup the block's state
            let latest = SealedHeader::new(latest_header, block_hash);

            PendingBlockEnvOrigin::DerivedFromLatest(latest)
        };

        let mut cfg = CfgEnvWithHandlerCfg::new_with_spec_id(CfgEnv::default(), SpecId::LATEST);

        let mut block_env = BlockEnv::default();
        // Note: for the PENDING block we assume it is past the known merge block and thus this will
        // not fail when looking up the total difficulty value for the blockenv.
        self.provider()
            .fill_env_with_header(
                &mut cfg,
                &mut block_env,
                origin.header(),
                self.evm_config().clone(),
            )
            .map_err(Self::Error::from_eth_err)?;

        Ok(PendingBlockEnv::new(cfg, block_env, origin))
    }

    /// Returns the locally built pending block
    fn local_pending_block(
        &self,
    ) -> impl Future<Output = Result<Option<(SealedBlockWithSenders, Vec<Receipt>)>, Self::Error>> + Send
    where
        Self: SpawnBlocking,
    {
        async move {
            let pending = self.pending_block_env_and_cfg()?;
            if pending.origin.is_actual_pending() {
                if let Some(block) = pending.origin.clone().into_actual_pending() {
                    // we have the real pending block, so we should also have its receipts
                    if let Some(receipts) = self
                        .provider()
                        .receipts_by_block(block.hash().into())
                        .map_err(Self::Error::from_eth_err)?
                    {
                        return Ok(Some((block, receipts)))
                    }
                }
            }

            // we couldn't find the real pending block, so we need to build it ourselves
            let mut lock = self.pending_block().lock().await;

            let now = Instant::now();

            // check if the block is still good
            if let Some(pending_block) = lock.as_ref() {
                // this is guaranteed to be the `latest` header
                if pending.block_env.number.to::<u64>() == pending_block.block.number &&
                    pending.origin.header().hash() == pending_block.block.parent_hash &&
                    now <= pending_block.expires_at
                {
                    return Ok(Some((pending_block.block.clone(), pending_block.receipts.clone())))
                }
            }

            // no pending block from the CL yet, so we need to build it ourselves via txpool
            let (sealed_block, receipts) = match self
                .spawn_blocking_io(move |this| {
                    // we rebuild the block
                    this.build_block(pending)
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

    /// Assembles a [`Receipt`] for a transaction, based on its [`ExecutionResult`].
    fn assemble_receipt(
        &self,
        tx: &TransactionSignedEcRecovered,
        result: ExecutionResult,
        cumulative_gas_used: u64,
    ) -> Receipt {
        #[allow(clippy::needless_update)]
        Receipt {
            tx_type: tx.tx_type(),
            success: result.is_success(),
            cumulative_gas_used,
            logs: result.into_logs().into_iter().map(Into::into).collect(),
            ..Default::default()
        }
    }

    /// Calculates receipts root in block building.
    ///
    /// Panics if block is not in the [`ExecutionOutcome`]'s block range.
    fn receipts_root(
        &self,
        _block_env: &BlockEnv,
        execution_outcome: &ExecutionOutcome,
        block_number: BlockNumber,
    ) -> B256 {
        execution_outcome.receipts_root_slow(block_number).expect("Block is present")
    }

    /// Builds a pending block using the configured provider and pool.
    ///
    /// If the origin is the actual pending block, the block is built with withdrawals.
    ///
    /// After Cancun, if the origin is the actual pending block, the block includes the EIP-4788 pre
    /// block contract call using the parent beacon block root received from the CL.
    fn build_block(
        &self,
        env: PendingBlockEnv,
    ) -> Result<(SealedBlockWithSenders, Vec<Receipt>), Self::Error>
    where
        EthApiError: From<ProviderError>,
    {
        let PendingBlockEnv { cfg, block_env, origin } = env;

        let parent_hash = origin.build_target_hash();
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

        let (withdrawals, withdrawals_root) = match origin {
            PendingBlockEnvOrigin::ActualPending(ref block) => {
                (block.body.withdrawals.clone(), block.withdrawals_root)
            }
            PendingBlockEnvOrigin::DerivedFromLatest(_) => (None, None),
        };

        let chain_spec = self.provider().chain_spec();

        let evm_config = self.evm_config().clone();
        let mut system_caller = SystemCaller::new(&evm_config, chain_spec.clone());

        let parent_beacon_block_root = if origin.is_actual_pending() {
            // apply eip-4788 pre block contract call if we got the block from the CL with the real
            // parent beacon block root
            system_caller
                .pre_block_beacon_root_contract_call(
                    &mut db,
                    &cfg,
                    &block_env,
                    origin.header().parent_beacon_block_root,
                )
                .map_err(|err| EthApiError::Internal(err.into()))?;
            origin.header().parent_beacon_block_root
        } else {
            None
        };
        system_caller
            .pre_block_blockhashes_contract_call(&mut db, &cfg, &block_env, origin.header().hash())
            .map_err(|err| EthApiError::Internal(err.into()))?;

        let mut receipts = Vec::new();

        while let Some(pool_tx) = best_txs.next() {
            // ensure we still have capacity for this transaction
            if cumulative_gas_used + pool_tx.gas_limit() > block_gas_limit {
                // we can't fit this transaction into the block, so we need to mark it as invalid
                // which also removes all dependent transaction from the iterator before we can
                // continue
                best_txs.mark_invalid(&pool_tx);
                continue
            }

            if pool_tx.origin.is_private() {
                // we don't want to leak any state changes made by private transactions, so we mark
                // them as invalid here which removes all dependent transactions from the iterator
                // before we can continue
                best_txs.mark_invalid(&pool_tx);
                continue
            }

            // convert tx to a signed transaction
            let tx = pool_tx.to_recovered_transaction();

            // There's only limited amount of blob space available per block, so we need to check if
            // the EIP-4844 can still fit in the block
            if let Some(blob_tx) = tx.transaction.as_eip4844() {
                let tx_blob_gas = blob_tx.blob_gas();
                if sum_blob_gas_used + tx_blob_gas > MAX_DATA_GAS_PER_BLOCK {
                    // we can't fit this _blob_ transaction into the block, so we mark it as
                    // invalid, which removes its dependent transactions from
                    // the iterator. This is similar to the gas limit condition
                    // for regular transactions above.
                    best_txs.mark_invalid(&pool_tx);
                    continue
                }
            }

            // Configure the environment for the block.
            let env = Env::boxed(
                cfg.cfg_env.clone(),
                block_env.clone(),
                Self::evm_config(self).tx_env(&tx),
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
                                best_txs.mark_invalid(&pool_tx);
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
            if let Some(blob_tx) = tx.transaction.as_eip4844() {
                let tx_blob_gas = blob_tx.blob_gas();
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
            &withdrawals.clone().unwrap_or_default(),
        );

        // increment account balances for withdrawals
        db.increment_balances(balance_increments).map_err(Self::Error::from_eth_err)?;

        // merge all transitions into bundle state.
        db.merge_transitions(BundleRetention::PlainState);

        let execution_outcome = ExecutionOutcome::new(
            db.take_bundle(),
            vec![receipts.clone()].into(),
            block_number,
            Vec::new(),
        );
        let hashed_state = HashedPostState::from_bundle_state(&execution_outcome.state().state);

        let receipts_root = self.receipts_root(&block_env, &execution_outcome, block_number);

        let logs_bloom =
            execution_outcome.block_logs_bloom(block_number).expect("Block is present");

        // calculate the state root
        let state_provider = &db.database;
        let state_root =
            state_provider.state_root(hashed_state).map_err(Self::Error::from_eth_err)?;

        // create the block header
        let transactions_root = calculate_transaction_root(&executed_txs);

        // check if cancun is activated to set eip4844 header fields correctly
        let blob_gas_used =
            if cfg.handler_cfg.spec_id >= SpecId::CANCUN { Some(sum_blob_gas_used) } else { None };

        // note(onbjerg): the rpc spec has not been changed to include requests, so for now we just
        // set these to empty
        let (requests, requests_root) =
            if chain_spec.is_prague_active_at_timestamp(block_env.timestamp.to::<u64>()) {
                (Some(Requests::default()), Some(EMPTY_ROOT_HASH))
            } else {
                (None, None)
            };

        let header = Header {
            parent_hash,
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: block_env.coinbase,
            state_root,
            transactions_root,
            receipts_root,
            withdrawals_root,
            logs_bloom,
            timestamp: block_env.timestamp.to::<u64>(),
            mix_hash: block_env.prevrandao.unwrap_or_default(),
            nonce: BEACON_NONCE.into(),
            base_fee_per_gas: Some(base_fee),
            number: block_number,
            gas_limit: block_gas_limit,
            difficulty: U256::ZERO,
            gas_used: cumulative_gas_used,
            blob_gas_used: blob_gas_used.map(Into::into),
            excess_blob_gas: block_env.get_blob_excess_gas().map(Into::into),
            extra_data: Default::default(),
            parent_beacon_block_root,
            requests_root,
        };

        // Convert Vec<Option<Receipt>> to Vec<Receipt>
        let receipts: Vec<Receipt> = receipts.into_iter().flatten().collect();

        // seal the block
        let block = Block {
            header,
            body: BlockBody { transactions: executed_txs, ommers: vec![], withdrawals, requests },
        };
        Ok((SealedBlockWithSenders { block: block.seal_slow(), senders }, receipts))
    }
}
