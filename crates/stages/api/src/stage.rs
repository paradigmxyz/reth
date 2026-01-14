use crate::{error::StageError, StageCheckpoint, StageId};
use alloy_primitives::{BlockNumber, TxNumber};
use reth_provider::{BlockReader, ProviderError, StaticFileProviderFactory, StaticFileSegment};
use std::{
    cmp::{max, min},
    future::{poll_fn, Future},
    ops::{Range, RangeInclusive},
    task::{Context, Poll},
};
use tracing::instrument;

/// Stage execution input, see [`Stage::execute`].
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct ExecInput {
    /// The target block number the stage needs to execute towards.
    pub target: Option<BlockNumber>,
    /// The checkpoint of this stage the last time it was executed.
    pub checkpoint: Option<StageCheckpoint>,
}

/// Return type for [`ExecInput::next_block_range_with_threshold`].
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct BlockRangeOutput {
    /// The block range to execute.
    pub block_range: RangeInclusive<BlockNumber>,
    /// Whether this is the final range to execute.
    pub is_final_range: bool,
}

/// Return type for [`ExecInput::next_block_range_with_transaction_threshold`].
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct TransactionRangeOutput {
    /// The transaction range to execute.
    pub tx_range: Range<TxNumber>,
    /// The block range to execute.
    pub block_range: RangeInclusive<BlockNumber>,
    /// Whether this is the final range to execute.
    pub is_final_range: bool,
}

impl ExecInput {
    /// Return the checkpoint of the stage or default.
    pub fn checkpoint(&self) -> StageCheckpoint {
        self.checkpoint.unwrap_or_default()
    }

    /// Return the next block number after the current
    /// +1 is needed to skip the present block and always start from block number 1, not 0.
    pub fn next_block(&self) -> BlockNumber {
        let current_block = self.checkpoint();
        current_block.block_number + 1
    }

    /// Returns `true` if the target block number has already been reached.
    pub fn target_reached(&self) -> bool {
        self.checkpoint().block_number >= self.target()
    }

    /// Return the target block number or default.
    pub fn target(&self) -> BlockNumber {
        self.target.unwrap_or_default()
    }

    /// Return next block range that needs to be executed.
    pub fn next_block_range(&self) -> RangeInclusive<BlockNumber> {
        self.next_block_range_with_threshold(u64::MAX).block_range
    }

    /// Return true if this is the first block range to execute.
    pub const fn is_first_range(&self) -> bool {
        self.checkpoint.is_none()
    }

    /// Return the next block range to execute.
    pub fn next_block_range_with_threshold(&self, threshold: u64) -> BlockRangeOutput {
        let current_block = self.checkpoint();
        let start = current_block.block_number + 1;
        let target = self.target();

        let end = min(target, current_block.block_number.saturating_add(threshold));

        let is_final_range = end == target;
        BlockRangeOutput { block_range: start..=end, is_final_range }
    }

    /// Return the next block range determined the number of transactions within it.
    /// This function walks the block indices until either the end of the range is reached or
    /// the number of transactions exceeds the threshold.
    ///
    /// Returns [`None`] if no transactions are found for the current execution input.
    #[instrument(level = "debug", target = "sync::stages", skip(provider), ret)]
    pub fn next_block_range_with_transaction_threshold<Provider>(
        &self,
        provider: &Provider,
        tx_threshold: u64,
    ) -> Result<Option<TransactionRangeOutput>, StageError>
    where
        Provider: StaticFileProviderFactory + BlockReader,
    {
        // Get lowest available block number for transactions
        let Some(lowest_transactions_block) =
            provider.static_file_provider().get_lowest_range_start(StaticFileSegment::Transactions)
        else {
            return Ok(None)
        };

        // We can only process transactions that have associated static files, so we cap the start
        // block by lowest available block number.
        //
        // Certain transactions may not have associated static files when user deletes them
        // manually. In that case, we can't process them, and need to adjust the start block
        // accordingly.
        let start_block = self.next_block().max(lowest_transactions_block);
        let target_block = self.target();

        // If the start block is greater than the target, then there's no transactions to process
        // and we return early. It's possible to trigger this scenario when running `reth
        // stage run` manually for a range of transactions that doesn't exist.
        if start_block > target_block {
            return Ok(None)
        }

        let start_block_body = provider
            .block_body_indices(start_block)?
            .ok_or(ProviderError::BlockBodyIndicesNotFound(start_block))?;
        let first_tx_num = start_block_body.first_tx_num();

        let target_block_body = provider
            .block_body_indices(target_block)?
            .ok_or(ProviderError::BlockBodyIndicesNotFound(target_block))?;

        // number of transactions left to execute.
        let all_tx_cnt = target_block_body.next_tx_num() - first_tx_num;

        if all_tx_cnt == 0 {
            // if there is no more transaction return back.
            return Ok(None)
        }

        // get block of this tx
        let (end_block, is_final_range, next_tx_num) = if all_tx_cnt <= tx_threshold {
            (target_block, true, target_block_body.next_tx_num())
        } else {
            // get tx block number. next_tx_num in this case will be less than all_tx_cnt.
            // So we are sure that transaction must exist.
            let end_block_number = provider
                .block_by_transaction_id(first_tx_num + tx_threshold)?
                .expect("block of tx must exist");
            // we want to get range of all transactions of this block, so we are fetching block
            // body.
            let end_block_body = provider
                .block_body_indices(end_block_number)?
                .ok_or(ProviderError::BlockBodyIndicesNotFound(end_block_number))?;
            (end_block_number, false, end_block_body.next_tx_num())
        };

        let tx_range = first_tx_num..next_tx_num;
        Ok(Some(TransactionRangeOutput {
            tx_range,
            block_range: start_block..=end_block,
            is_final_range,
        }))
    }
}

/// Stage unwind input, see [`Stage::unwind`].
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct UnwindInput {
    /// The current highest checkpoint of the stage.
    pub checkpoint: StageCheckpoint,
    /// The block to unwind to.
    pub unwind_to: BlockNumber,
    /// The bad block that caused the unwind, if any.
    pub bad_block: Option<BlockNumber>,
}

impl UnwindInput {
    /// Return next block range that needs to be unwound.
    pub fn unwind_block_range(&self) -> RangeInclusive<BlockNumber> {
        self.unwind_block_range_with_threshold(u64::MAX).0
    }

    /// Return the next block range to unwind and the block we're unwinding to.
    pub fn unwind_block_range_with_threshold(
        &self,
        threshold: u64,
    ) -> (RangeInclusive<BlockNumber>, BlockNumber, bool) {
        // +1 is to skip the block we're unwinding to
        let mut start = self.unwind_to + 1;
        let end = self.checkpoint;

        start = max(start, end.block_number.saturating_sub(threshold));

        let unwind_to = start - 1;

        let is_final_range = unwind_to == self.unwind_to;
        (start..=end.block_number, unwind_to, is_final_range)
    }
}

/// The output of a stage execution.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ExecOutput {
    /// How far the stage got.
    pub checkpoint: StageCheckpoint,
    /// Whether or not the stage is done.
    pub done: bool,
}

impl ExecOutput {
    /// Mark the stage as not done, checkpointing at the given place.
    pub const fn in_progress(checkpoint: StageCheckpoint) -> Self {
        Self { checkpoint, done: false }
    }

    /// Mark the stage as done, checkpointing at the given place.
    pub const fn done(checkpoint: StageCheckpoint) -> Self {
        Self { checkpoint, done: true }
    }
}

/// The output of a stage unwinding.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct UnwindOutput {
    /// The checkpoint at which the stage has unwound to.
    pub checkpoint: StageCheckpoint,
}

/// A stage is a segmented part of the syncing process of the node.
///
/// Each stage takes care of a well-defined task, such as downloading headers or executing
/// transactions, and persist their results to a database.
///
/// Stages must have a unique [ID][StageId] and implement a way to "roll forwards"
/// ([`Stage::execute`]) and a way to "roll back" ([`Stage::unwind`]).
///
/// Stages are executed as part of a pipeline where they are executed serially.
///
/// Stages receive [`DBProvider`](reth_provider::DBProvider).
#[auto_impl::auto_impl(Box)]
pub trait Stage<Provider>: Send {
    /// Get the ID of the stage.
    ///
    /// Stage IDs must be unique.
    fn id(&self) -> StageId;

    /// Returns `Poll::Ready(Ok(()))` when the stage is ready to execute the given range.
    ///
    /// This method is heavily inspired by [tower](https://crates.io/crates/tower)'s `Service` trait.
    /// Any asynchronous tasks or communication should be handled in `poll_execute_ready`, e.g.
    /// moving downloaded items from downloaders to an internal buffer in the stage.
    ///
    /// If the stage has any pending external state, then `Poll::Pending` is returned.
    ///
    /// If `Poll::Ready(Err(_))` is returned, the stage may not be able to execute anymore
    /// depending on the specific error. In that case, an unwind must be issued instead.
    ///
    /// Once `Poll::Ready(Ok(()))` is returned, the stage may be executed once using `execute`.
    /// Until the stage has been executed, repeated calls to `poll_execute_ready` must return either
    /// `Poll::Ready(Ok(()))` or `Poll::Ready(Err(_))`.
    ///
    /// Note that `poll_execute_ready` may reserve shared resources that are consumed in a
    /// subsequent call of `execute`, e.g. internal buffers. It is crucial for implementations
    /// to not assume that `execute` will always be invoked and to ensure that those resources
    /// are appropriately released if the stage is dropped before `execute` is called.
    ///
    /// For the same reason, it is also important that any shared resources do not exhibit
    /// unbounded growth on repeated calls to `poll_execute_ready`.
    ///
    /// Unwinds may happen without consulting `poll_execute_ready` first.
    fn poll_execute_ready(
        &mut self,
        _cx: &mut Context<'_>,
        _input: ExecInput,
    ) -> Poll<Result<(), StageError>> {
        Poll::Ready(Ok(()))
    }

    /// Execute the stage.
    /// It is expected that the stage will write all necessary data to the database
    /// upon invoking this method.
    fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError>;

    /// Post execution commit hook.
    ///
    /// This is called after the stage has been executed and the data has been committed by the
    /// provider. The stage may want to pass some data from [`Self::execute`] via the internal
    /// field.
    fn post_execute_commit(&mut self) -> Result<(), StageError> {
        Ok(())
    }

    /// Unwind the stage.
    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError>;

    /// Post unwind commit hook.
    ///
    /// This is called after the stage has been unwound and the data has been committed by the
    /// provider. The stage may want to pass some data from [`Self::unwind`] via the internal
    /// field.
    fn post_unwind_commit(&mut self) -> Result<(), StageError> {
        Ok(())
    }
}

/// [Stage] trait extension.
pub trait StageExt<Provider>: Stage<Provider> {
    /// Utility extension for the `Stage` trait that invokes `Stage::poll_execute_ready`
    /// with [`poll_fn`] context. For more information see [`Stage::poll_execute_ready`].
    fn execute_ready(
        &mut self,
        input: ExecInput,
    ) -> impl Future<Output = Result<(), StageError>> + Send {
        poll_fn(move |cx| self.poll_execute_ready(cx, input))
    }
}

impl<Provider, S: Stage<Provider> + ?Sized> StageExt<Provider> for S {}

#[cfg(test)]
mod tests {
    use reth_chainspec::MAINNET;
    use reth_db::test_utils::{
        create_test_rocksdb_dir, create_test_rw_db, create_test_static_files_dir,
    };
    use reth_db_api::{models::StoredBlockBodyIndices, tables, transaction::DbTxMut};
    use reth_provider::{
        providers::RocksDBProvider, test_utils::MockNodeTypesWithDB, ProviderFactory,
        StaticFileProviderBuilder, StaticFileProviderFactory, StaticFileSegment,
    };
    use reth_stages_types::StageCheckpoint;
    use reth_testing_utils::generators::{self, random_signed_tx};

    use crate::ExecInput;

    #[test]
    fn test_exec_input_next_block_range_with_transaction_threshold() {
        let mut rng = generators::rng();
        let provider_factory = ProviderFactory::<MockNodeTypesWithDB>::new(
            create_test_rw_db(),
            MAINNET.clone(),
            StaticFileProviderBuilder::read_write(create_test_static_files_dir().0.keep())
                .with_blocks_per_file(1)
                .build()
                .unwrap(),
            RocksDBProvider::builder(create_test_rocksdb_dir().0.keep())
                .with_default_tables()
                .build()
                .unwrap(),
        )
        .unwrap();

        // Without checkpoint, without transactions in static files
        {
            let exec_input = ExecInput { target: Some(100), checkpoint: None };

            let range_output = exec_input
                .next_block_range_with_transaction_threshold(&provider_factory, 10)
                .unwrap();
            assert!(range_output.is_none());
        }

        // With checkpoint at block 10, without transactions in static files
        {
            let exec_input =
                ExecInput { target: Some(1), checkpoint: Some(StageCheckpoint::new(10)) };

            let range_output = exec_input
                .next_block_range_with_transaction_threshold(&provider_factory, 10)
                .unwrap();
            assert!(range_output.is_none());
        }

        // Without checkpoint, with transactions in static files starting from block 1
        {
            let exec_input = ExecInput { target: Some(1), checkpoint: None };

            let mut provider_rw = provider_factory.provider_rw().unwrap();
            provider_rw
                .tx_mut()
                .put::<tables::BlockBodyIndices>(
                    1,
                    StoredBlockBodyIndices { first_tx_num: 0, tx_count: 2 },
                )
                .unwrap();
            let mut writer =
                provider_rw.get_static_file_writer(0, StaticFileSegment::Transactions).unwrap();
            writer.increment_block(0).unwrap();
            writer.increment_block(1).unwrap();
            writer.append_transaction(0, &random_signed_tx(&mut rng)).unwrap();
            writer.append_transaction(1, &random_signed_tx(&mut rng)).unwrap();
            drop(writer);
            provider_rw.commit().unwrap();

            let range_output = exec_input
                .next_block_range_with_transaction_threshold(&provider_factory, 10)
                .unwrap()
                .unwrap();
            assert_eq!(range_output.tx_range, 0..2);
            assert_eq!(range_output.block_range, 1..=1);
            assert!(range_output.is_final_range);
        }

        // With checkpoint at block 1, with transactions in static files starting from block 1
        {
            let exec_input =
                ExecInput { target: Some(2), checkpoint: Some(StageCheckpoint::new(1)) };

            let mut provider_rw = provider_factory.provider_rw().unwrap();
            provider_rw
                .tx_mut()
                .put::<tables::BlockBodyIndices>(
                    2,
                    StoredBlockBodyIndices { first_tx_num: 2, tx_count: 1 },
                )
                .unwrap();
            let mut writer =
                provider_rw.get_static_file_writer(1, StaticFileSegment::Transactions).unwrap();
            writer.increment_block(2).unwrap();
            writer.append_transaction(2, &random_signed_tx(&mut rng)).unwrap();
            drop(writer);
            provider_rw.commit().unwrap();

            let range_output = exec_input
                .next_block_range_with_transaction_threshold(&provider_factory, 10)
                .unwrap()
                .unwrap();
            assert_eq!(range_output.tx_range, 2..3);
            assert_eq!(range_output.block_range, 2..=2);
            assert!(range_output.is_final_range);
        }

        // Without checkpoint, with transactions in static files starting from block 2
        {
            let exec_input = ExecInput { target: Some(2), checkpoint: None };

            provider_factory
                .static_file_provider()
                .delete_jar(StaticFileSegment::Transactions, 0)
                .unwrap();
            provider_factory
                .static_file_provider()
                .delete_jar(StaticFileSegment::Transactions, 1)
                .unwrap();

            let range_output = exec_input
                .next_block_range_with_transaction_threshold(&provider_factory, 10)
                .unwrap()
                .unwrap();
            assert_eq!(range_output.tx_range, 2..3);
            assert_eq!(range_output.block_range, 2..=2);
            assert!(range_output.is_final_range);
        }

        // Without checkpoint, with transactions in static files starting from block 2
        {
            let exec_input =
                ExecInput { target: Some(3), checkpoint: Some(StageCheckpoint::new(2)) };

            let mut provider_rw = provider_factory.provider_rw().unwrap();
            provider_rw
                .tx_mut()
                .put::<tables::BlockBodyIndices>(
                    3,
                    StoredBlockBodyIndices { first_tx_num: 3, tx_count: 1 },
                )
                .unwrap();
            let mut writer =
                provider_rw.get_static_file_writer(1, StaticFileSegment::Transactions).unwrap();
            writer.increment_block(3).unwrap();
            writer.append_transaction(3, &random_signed_tx(&mut rng)).unwrap();
            drop(writer);
            provider_rw.commit().unwrap();

            let range_output = exec_input
                .next_block_range_with_transaction_threshold(&provider_factory, 10)
                .unwrap()
                .unwrap();
            assert_eq!(range_output.tx_range, 3..4);
            assert_eq!(range_output.block_range, 3..=3);
            assert!(range_output.is_final_range);
        }
    }
}
