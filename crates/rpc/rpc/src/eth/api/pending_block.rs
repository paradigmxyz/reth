//! Support for building a pending block via local txpool.

use crate::eth::error::EthResult;
use reth_primitives::{
    constants::{BEACON_NONCE, EMPTY_WITHDRAWALS},
    proofs, Block, Header, IntoRecoveredTransaction, Receipt, SealedBlock, SealedHeader,
    EMPTY_OMMER_ROOT, H256, U256,
};
use reth_provider::{PostState, StateProviderFactory};
use reth_revm::{
    database::State, env::tx_env_with_recovered, executor::commit_state_changes, into_reth_log,
};
use reth_transaction_pool::TransactionPool;
use revm::db::CacheDB;
use revm_primitives::{BlockEnv, CfgEnv, EVMError, Env, InvalidTransaction, ResultAndState};
use std::time::Instant;

/// Configured [BlockEnv] and [CfgEnv] for a pending block
#[derive(Debug, Clone)]
pub(crate) struct PendingBlockEnv {
    /// Configured [CfgEnv] for the pending block.
    pub(crate) cfg: CfgEnv,
    /// Configured [BlockEnv] for the pending block.
    pub(crate) block_env: BlockEnv,
    /// Origin block for the config
    pub(crate) origin: PendingBlockEnvOrigin,
}

impl PendingBlockEnv {
    /// Builds a pending block from the given client and pool.
    pub(crate) fn build_block<Client, Pool>(
        self,
        client: &Client,
        pool: &Pool,
    ) -> EthResult<SealedBlock>
    where
        Client: StateProviderFactory,
        Pool: TransactionPool,
    {
        let Self { cfg, block_env, origin } = self;

        let parent_hash = origin.build_target_hash();
        let state = State::new(client.history_by_block_hash(parent_hash)?);
        let mut db = CacheDB::new(state);
        let mut post_state = PostState::default();

        let mut cumulative_gas_used = 0;
        let block_gas_limit: u64 = block_env.gas_limit.try_into().unwrap_or(u64::MAX);
        let base_fee = block_env.basefee.to::<u64>();
        let block_number = block_env.number.to::<u64>();

        let mut executed_txs = Vec::new();
        let mut best_txs = pool.best_transactions_with_base_fee(base_fee as u128);

        while let Some(pool_tx) = best_txs.next() {
            // ensure we still have capacity for this transaction
            if cumulative_gas_used + pool_tx.gas_limit() > block_gas_limit {
                // we can't fit this transaction into the block, so we need to mark it as invalid
                // which also removes all dependent transaction from the iterator before we can
                // continue
                best_txs.mark_invalid(&pool_tx);
                continue
            }

            // convert tx to a signed transaction
            let tx = pool_tx.to_recovered_transaction();

            // Configure the environment for the block.
            let env =
                Env { cfg: cfg.clone(), block: block_env.clone(), tx: tx_env_with_recovered(&tx) };

            let mut evm = revm::EVM::with_env(env);
            evm.database(&mut db);

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
                            return Err(err.into())
                        }
                    }
                }
            };

            let gas_used = result.gas_used();

            // commit changes
            commit_state_changes(&mut db, &mut post_state, block_number, state, true);

            // add gas used by the transaction to cumulative gas used, before creating the receipt
            cumulative_gas_used += gas_used;

            // Push transaction changeset and calculate header bloom filter for receipt.
            post_state.add_receipt(
                block_number,
                Receipt {
                    tx_type: tx.tx_type(),
                    success: result.is_success(),
                    cumulative_gas_used,
                    logs: result.logs().into_iter().map(into_reth_log).collect(),
                },
            );
            // append transaction to the list of executed transactions
            executed_txs.push(tx.into_signed());
        }

        let receipts_root = post_state.receipts_root(block_number);
        let logs_bloom = post_state.logs_bloom(block_number);

        // calculate the state root
        let state_root = db.db.state().state_root(post_state)?;

        // create the block header
        let transactions_root = proofs::calculate_transaction_root(&executed_txs);

        let header = Header {
            parent_hash,
            ommers_hash: EMPTY_OMMER_ROOT,
            beneficiary: block_env.coinbase,
            state_root,
            transactions_root,
            receipts_root,
            withdrawals_root: Some(EMPTY_WITHDRAWALS),
            logs_bloom,
            timestamp: block_env.timestamp.to::<u64>(),
            mix_hash: block_env.prevrandao.unwrap_or_default(),
            nonce: BEACON_NONCE,
            base_fee_per_gas: Some(base_fee),
            number: block_number,
            gas_limit: block_gas_limit,
            difficulty: U256::ZERO,
            gas_used: cumulative_gas_used,
            extra_data: Default::default(),
        };

        // seal the block
        let block = Block { header, body: executed_txs, ommers: vec![], withdrawals: Some(vec![]) };
        let sealed_block = block.seal_slow();

        Ok(sealed_block)
    }
}

/// The origin for a configured [PendingBlockEnv]
#[derive(Clone, Debug)]
pub(crate) enum PendingBlockEnvOrigin {
    /// The pending block as received from the CL.
    ActualPending(SealedBlock),
    /// The header of the latest block
    DerivedFromLatest(SealedHeader),
}

impl PendingBlockEnvOrigin {
    /// Returns true if the origin is the actual pending block as received from the CL.
    pub(crate) fn is_actual_pending(&self) -> bool {
        matches!(self, PendingBlockEnvOrigin::ActualPending(_))
    }

    /// Consumes the type and returns the actual pending block.
    pub(crate) fn into_actual_pending(self) -> Option<SealedBlock> {
        match self {
            PendingBlockEnvOrigin::ActualPending(block) => Some(block),
            _ => None,
        }
    }

    /// Returns the hash of the pending block should be built on
    fn build_target_hash(&self) -> H256 {
        match self {
            PendingBlockEnvOrigin::ActualPending(block) => block.parent_hash,
            PendingBlockEnvOrigin::DerivedFromLatest(header) => header.hash,
        }
    }

    /// Returns the header this pending block is based on.
    pub(crate) fn header(&self) -> &SealedHeader {
        match self {
            PendingBlockEnvOrigin::ActualPending(block) => &block.header,
            PendingBlockEnvOrigin::DerivedFromLatest(header) => header,
        }
    }
}

/// In memory pending block for `pending` tag
#[derive(Debug)]
pub(crate) struct PendingBlock {
    /// The cached pending block
    pub(crate) block: SealedBlock,
    /// Timestamp when the pending block is considered outdated
    pub(crate) expires_at: Instant,
}
