use crate::error::StageError;
use reth_db::database::Database;
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    BlockNumber, TxNumber,
};
use reth_provider::{BlockReader, DatabaseProviderRW, ProviderError, TransactionsProvider};
use std::{
    cmp::{max, min},
    future::{poll_fn, Future},
    ops::{Range, RangeInclusive},
    task::{Context, Poll},
};

/// Stage execution input, see [Stage::execute].
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct ExecInput {
    /// The target block number the stage needs to execute towards.
    pub target: Option<BlockNumber>,
    /// The checkpoint of this stage the last time it was executed.
    pub checkpoint: Option<StageCheckpoint>,
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
        let (range, _) = self.next_block_range_with_threshold(u64::MAX);
        range
    }

    /// Return true if this is the first block range to execute.
    pub fn is_first_range(&self) -> bool {
        self.checkpoint.is_none()
    }

    /// Return the next block range to execute.
    /// Return pair of the block range and if this is final block range.
    pub fn next_block_range_with_threshold(
        &self,
        threshold: u64,
    ) -> (RangeInclusive<BlockNumber>, bool) {
        let current_block = self.checkpoint();
        let start = current_block.block_number + 1;
        let target = self.target();

        let end = min(target, current_block.block_number.saturating_add(threshold));

        let is_final_range = end == target;
        (start..=end, is_final_range)
    }

    /// Return the next block range determined the number of transactions within it.
    /// This function walks the block indices until either the end of the range is reached or
    /// the number of transactions exceeds the threshold.
    pub fn next_block_range_with_transaction_threshold<DB: Database>(
        &self,
        provider: &DatabaseProviderRW<DB>,
        tx_threshold: u64,
    ) -> Result<(Range<TxNumber>, RangeInclusive<BlockNumber>, bool), StageError> {
        let start_block = self.next_block();
        let target_block = self.target();

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
            return Ok((first_tx_num..first_tx_num, start_block..=target_block, true))
        }

        // get block of this tx
        let (end_block, is_final_range, next_tx_num) = if all_tx_cnt <= tx_threshold {
            (target_block, true, target_block_body.next_tx_num())
        } else {
            // get tx block number. next_tx_num in this case will be less thean all_tx_cnt.
            // So we are sure that transaction must exist.
            let end_block_number = provider
                .transaction_block(first_tx_num + tx_threshold)?
                .expect("block of tx must exist");
            // we want to get range of all transactions of this block, so we are fetching block
            // body.
            let end_block_body = provider
                .block_body_indices(end_block_number)?
                .ok_or(ProviderError::BlockBodyIndicesNotFound(target_block))?;
            (end_block_number, false, end_block_body.next_tx_num())
        };

        let tx_range = first_tx_num..next_tx_num;
        Ok((tx_range, start_block..=end_block, is_final_range))
    }
}

/// Stage unwind input, see [Stage::unwind].
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
    /// Mark the stage as done, checkpointing at the given place.
    pub fn done(checkpoint: StageCheckpoint) -> Self {
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
/// ([Stage::execute]) and a way to "roll back" ([Stage::unwind]).
///
/// Stages are executed as part of a pipeline where they are executed serially.
///
/// Stages receive [`DatabaseProviderRW`].
#[auto_impl::auto_impl(Box)]
pub trait Stage<DB: Database>: Send + Sync {
    /// Get the ID of the stage.
    ///
    /// Stage IDs must be unique.
    fn id(&self) -> StageId;

    /// Returns `Poll::Ready(Ok(()))` when the stage is ready to execute the given range.
    ///
    /// This method is heavily inspired by [tower](https://crates.io/crates/tower)'s `Service` trait.
    /// Any asynchronous tasks or communication should be handled in `poll_ready`, e.g. moving
    /// downloaded items from downloaders to an internal buffer in the stage.
    ///
    /// If the stage has any pending external state, then `Poll::Pending` is returned.
    ///
    /// If `Poll::Ready(Err(_))` is returned, the stage may not be able to execute anymore
    /// depending on the specific error. In that case, an unwind must be issued instead.
    ///
    /// Once `Poll::Ready(Ok(()))` is returned, the stage may be executed once using `execute`.
    /// Until the stage has been executed, repeated calls to `poll_ready` must return either
    /// `Poll::Ready(Ok(()))` or `Poll::Ready(Err(_))`.
    ///
    /// Note that `poll_ready` may reserve shared resources that are consumed in a subsequent call
    /// of `execute`, e.g. internal buffers. It is crucial for implementations to not assume that
    /// `execute` will always be invoked and to ensure that those resources are appropriately
    /// released if the stage is dropped before `execute` is called.
    ///
    /// For the same reason, it is also important that any shared resources do not exhibit
    /// unbounded growth on repeated calls to `poll_ready`.
    ///
    /// Unwinds may happen without consulting `poll_ready` first.
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
    fn execute(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError>;

    /// Unwind the stage.
    fn unwind(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError>;
}

/// [Stage] trait extension.
pub trait StageExt<DB: Database>: Stage<DB> {
    /// Utility extension for the `Stage` trait that invokes `Stage::poll_execute_ready`
    /// with [poll_fn] context. For more information see [Stage::poll_execute_ready].
    fn execute_ready(
        &mut self,
        input: ExecInput,
    ) -> impl Future<Output = Result<(), StageError>> + Send {
        poll_fn(move |cx| self.poll_execute_ready(cx, input))
    }
}

impl<DB: Database, S: Stage<DB>> StageExt<DB> for S {}
