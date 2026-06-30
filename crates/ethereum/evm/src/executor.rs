//! EVM-backed Ethereum executor.

use crate::{
    execution::{
        block_requests_from_receipts, execute_transaction, post_block_balance_state_changes,
        post_execution_system_call_state_changes, pre_execution_system_call_state_changes,
    },
    BlockExecutionContext, BlockSystemCalls, EthBlockExecutionCtx, EthTxEnv, HashedStateMode,
    RethReceiptBuilder,
};
use alloc::{borrow::Cow, vec::Vec};
use alloy_consensus::{Header, TxType};
use alloy_eips::{eip2718::Typed2718, eip4895::Withdrawal};
use alloy_primitives::{Address, B256};
use evm2::{
    evm::{BlockStateAccumulator, Database},
    interpreter::Host,
    BaseEvmTypes, Evm,
};
use reth_ethereum_primitives::{EthPrimitives, Receipt};
use reth_evm::execute::{BlockExecutionError, BlockExecutionOutput, BlockExecutor, GasOutput};
use reth_execution_types::HashedPostStateSink;
use reth_trie_common::{HashedPostState, KeccakKeyHasher};

/// Configured Ethereum block executor backed by evm2.
#[expect(missing_debug_implementations)]
pub struct EthBlockExecutor<'a, DB> {
    evm: Evm<BaseEvmTypes>,
    spec_id: evm2::SpecId,
    block_number: u64,
    block_beneficiary: Address,
    parent_hash: B256,
    parent_beacon_block_root: Option<B256>,
    ommers: &'a [Header],
    withdrawals: Option<Cow<'a, [Withdrawal]>>,
    chain_id: u64,
    deposit_contract_address: Option<Address>,
    block_state: BlockStateAccumulator,
    hashed_state: Option<HashedPostStateSink<KeccakKeyHasher>>,
    hashed_state_mode: HashedStateMode,
    receipts: Vec<Receipt>,
    cumulative_gas_used: u64,
    _database: core::marker::PhantomData<DB>,
}

impl<'a, DB> EthBlockExecutor<'a, DB>
where
    DB: Database + 'static,
{
    /// Creates a configured Ethereum block executor.
    pub(crate) fn new(
        mut evm: Evm<BaseEvmTypes>,
        context: EthBlockExecutionCtx<'a>,
        chain_id: u64,
        deposit_contract_address: Option<alloy_primitives::Address>,
        hashed_state_mode: HashedStateMode,
    ) -> Self {
        let spec_id = evm.spec_id();
        let block = *evm.block_env();
        let block_number = block.number.to::<u64>();
        let block_beneficiary = block.beneficiary;

        Self {
            evm,
            spec_id,
            block_number,
            block_beneficiary,
            parent_hash: context.parent_hash,
            parent_beacon_block_root: context.parent_beacon_block_root,
            ommers: context.ommers,
            withdrawals: context.withdrawals,
            chain_id,
            deposit_contract_address,
            block_state: BlockStateAccumulator::new(),
            hashed_state: hashed_state_mode
                .output()
                .then(HashedPostStateSink::<KeccakKeyHasher>::default),
            hashed_state_mode,
            receipts: Vec::new(),
            cumulative_gas_used: 0,
            _database: core::marker::PhantomData,
        }
    }

    const fn block_context<'ctx>(
        chain_id: u64,
        deposit_contract_address: Option<Address>,
        parent_hash: B256,
        parent_beacon_block_root: Option<B256>,
        ommers: &'ctx [Header],
        withdrawals: Option<&'ctx [Withdrawal]>,
    ) -> BlockExecutionContext<'ctx> {
        BlockExecutionContext {
            chain_id,
            system_calls: Some(BlockSystemCalls { parent_hash, parent_beacon_block_root }),
            ommers: Some(ommers),
            withdrawals,
            deposit_contract_address,
        }
    }
}

impl<DB> BlockExecutor for EthBlockExecutor<'_, DB>
where
    DB: Database + 'static,
    DB::Error: core::error::Error + Send + Sync + 'static,
{
    type Primitives = EthPrimitives;
    type Transaction = EthTxEnv;
    type TransactionOutput = GasOutput;
    type Error = BlockExecutionError;

    fn evm(&self) -> &Evm<BaseEvmTypes> {
        &self.evm
    }

    fn evm_mut(&mut self) -> &mut Evm<BaseEvmTypes> {
        &mut self.evm
    }

    fn apply_pre_execution_changes<H>(
        &mut self,
        on_hashed_state_update: &mut H,
    ) -> Result<(), Self::Error>
    where
        H: FnMut(HashedPostState),
    {
        let context = Self::block_context(
            self.chain_id,
            self.deposit_contract_address,
            self.parent_hash,
            self.parent_beacon_block_root,
            self.ommers,
            None,
        );
        pre_execution_system_call_state_changes::<DB>(
            &mut self.evm,
            &mut self.block_state,
            self.hashed_state.as_mut(),
            self.hashed_state_mode.stream(),
            on_hashed_state_update,
            self.spec_id,
            self.block_number,
            context,
        )
        .map_err(Into::into)
    }

    fn execute_transaction<H>(
        &mut self,
        transaction: Self::Transaction,
        on_hashed_state_update: &mut H,
    ) -> Result<Self::TransactionOutput, Self::Error>
    where
        H: FnMut(HashedPostState),
    {
        let tx_hash = transaction.tx_hash();
        let transaction = transaction.into_envelope();
        let tx_type =
            TxType::try_from(transaction.ty()).expect("transaction envelope has valid type");
        let outcome = execute_transaction::<DB>(
            &mut self.evm,
            &mut self.block_state,
            self.hashed_state.as_mut(),
            self.hashed_state_mode.stream(),
            on_hashed_state_update,
            &transaction,
        )
        .map_err(|err| BlockExecutionError::evm(err, tx_hash))?;
        let gas_used = outcome.gas_used;
        self.cumulative_gas_used += gas_used;
        self.receipts.push(RethReceiptBuilder.build_receipt(
            tx_type,
            outcome,
            self.cumulative_gas_used,
        ));
        Ok(GasOutput::from(gas_used))
    }

    fn receipts(&self) -> &[Receipt] {
        &self.receipts
    }

    fn finish<H>(
        mut self,
        on_hashed_state_update: &mut H,
    ) -> Result<BlockExecutionOutput<Receipt>, Self::Error>
    where
        H: FnMut(HashedPostState),
    {
        let context = Self::block_context(
            self.chain_id,
            self.deposit_contract_address,
            self.parent_hash,
            self.parent_beacon_block_root,
            self.ommers,
            None,
        );
        let mut requests =
            block_requests_from_receipts::<DB>(self.spec_id, context, &self.receipts)?;
        let context = Self::block_context(
            self.chain_id,
            self.deposit_contract_address,
            self.parent_hash,
            self.parent_beacon_block_root,
            self.ommers,
            None,
        );
        post_execution_system_call_state_changes::<DB>(
            &mut self.evm,
            &mut self.block_state,
            self.hashed_state.as_mut(),
            self.hashed_state_mode.stream(),
            on_hashed_state_update,
            self.spec_id,
            context,
            &mut requests,
        )
        .map_err(BlockExecutionError::from)?;

        let withdrawals = self.withdrawals.clone();
        let context = Self::block_context(
            self.chain_id,
            self.deposit_contract_address,
            self.parent_hash,
            self.parent_beacon_block_root,
            self.ommers,
            withdrawals.as_deref(),
        );
        post_block_balance_state_changes::<DB>(
            &mut self.evm,
            &mut self.block_state,
            self.hashed_state.as_mut(),
            self.hashed_state_mode.stream(),
            on_hashed_state_update,
            self.spec_id,
            self.block_number,
            self.block_beneficiary,
            context.ommers,
            context.withdrawals,
        )
        .map_err(BlockExecutionError::from)?;

        let mut output = RethReceiptBuilder
            .build_block_output_from_receipts_and_state_with_hashed_state(
                self.receipts,
                self.block_state,
                self.hashed_state.map(HashedPostStateSink::into_hashed_post_state),
            );
        output.result.requests = requests;

        Ok(output)
    }
}
