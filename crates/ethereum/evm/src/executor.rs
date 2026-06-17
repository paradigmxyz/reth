//! evm2-backed Ethereum executor.

use crate::{
    evm2_block_env_with_blob_params, evm2_spec, execute_evm2_block_with_context_and_precompiles,
    Evm2BlockExecutionContext, Evm2BlockSystemCalls, Evm2ExecutionError,
};
use alloc::{sync::Arc, vec::Vec};
use alloy_consensus::{transaction::Recovered, Header};
use alloy_primitives::Bytes;
use evm2::evm::Database;
use reth_chainspec::{EthChainSpec, EthExecutorSpec};
use reth_ethereum_forks::Hardforks;
use reth_ethereum_primitives::{Block, EthPrimitives, Receipt, TransactionSigned};
use reth_evm::{
    evm2_precompile_cache::{CachedPrecompileProvider, PrecompileCacheMap},
    execute::{BlockExecutionOutput, ExecutionOutcome, Executor},
};
use reth_execution_types::BlockExecutionResult;
use reth_primitives_traits::{BlockBody, RecoveredBlock};

/// evm2-backed Ethereum block executor.
#[derive(Debug, Clone)]
pub struct EthEvm2Executor<C, DB> {
    chain_spec: Arc<C>,
    database: DB,
    precompile_cache_map: PrecompileCacheMap<evm2::SpecId>,
}

impl<C, DB> EthEvm2Executor<C, DB> {
    /// Creates a new evm2-backed Ethereum executor.
    pub const fn new(
        chain_spec: Arc<C>,
        database: DB,
        precompile_cache_map: PrecompileCacheMap<evm2::SpecId>,
    ) -> Self {
        Self { chain_spec, database, precompile_cache_map }
    }
}

impl<C, DB> EthEvm2Executor<C, DB>
where
    C: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
    DB: Database + Clone + 'static,
    DB::Error: core::error::Error + Send + Sync + 'static,
{
    fn execute_block(
        &self,
        block: &RecoveredBlock<Block>,
    ) -> Result<BlockExecutionOutput<Receipt>, Evm2ExecutionError<DB::Error>> {
        let header = block.header();
        let transactions = block
            .senders_iter()
            .zip(block.body().transactions())
            .map(|(signer, tx)| Recovered::new_unchecked(tx.clone(), *signer))
            .collect::<Vec<TransactionSignedWithSigner>>();
        let context = Evm2BlockExecutionContext {
            chain_id: self.chain_spec.chain_id(),
            system_calls: Some(Evm2BlockSystemCalls {
                parent_hash: header.parent_hash,
                parent_beacon_block_root: header.parent_beacon_block_root,
            }),
            ommers: Some(&block.body().ommers),
            withdrawals: block.body().withdrawals().map(|withdrawals| withdrawals.as_slice()),
            deposit_contract_address: self.chain_spec.deposit_contract_address(),
        };
        let spec_id = evm2_spec(self.chain_spec.as_ref(), header);

        execute_evm2_block_with_context_and_precompiles(
            spec_id,
            evm2_block_env_with_blob_params(
                header,
                self.chain_spec.as_ref().blob_params_at_timestamp(header.timestamp),
            ),
            self.database.clone(),
            header.number,
            transactions,
            context,
            alloc::boxed::Box::new(CachedPrecompileProvider::new(
                evm2::Precompiles::base(spec_id),
                self.precompile_cache_map.clone(),
                spec_id,
                None,
            )),
        )
    }
}

impl<C, DB> Executor for EthEvm2Executor<C, DB>
where
    C: EthExecutorSpec + EthChainSpec<Header = Header> + Hardforks + 'static,
    DB: Database + Clone + 'static,
    DB::Error: core::error::Error + Send + Sync + 'static,
{
    type Primitives = EthPrimitives;
    type Error = Evm2ExecutionError<DB::Error>;

    fn execute_one(
        &mut self,
        block: &RecoveredBlock<Block>,
    ) -> Result<BlockExecutionResult<Receipt>, Self::Error> {
        self.execute_block(block).map(|output| output.result)
    }

    fn execute(
        self,
        block: &RecoveredBlock<Block>,
    ) -> Result<BlockExecutionOutput<Receipt>, Self::Error> {
        self.execute_block(block)
    }

    fn execute_batch<'a, I>(self, blocks: I) -> Result<ExecutionOutcome<Receipt>, Self::Error>
    where
        I: IntoIterator<Item = &'a RecoveredBlock<Block>>,
    {
        let mut blocks = blocks.into_iter();
        let Some(block) = blocks.next() else { return Ok(ExecutionOutcome::default()) };
        if blocks.next().is_some() {
            return Err(Evm2ExecutionError::UnsupportedBatchExecution)
        }

        let block_number = block.header().number;
        let output = self.execute_block(block)?;
        Ok(ExecutionOutcome::single(block_number, output))
    }

    fn size_hint(&self) -> usize {
        0
    }

    fn take_bal(&mut self) -> Option<Bytes> {
        None
    }
}

type TransactionSignedWithSigner = Recovered<TransactionSigned>;
