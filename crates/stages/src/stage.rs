use std::{
    future::{poll_fn, Future},
    task::{Context, Poll},
};

use reth_db::database::Database;
use reth_primitives::stage::StageId;
use reth_provider::DatabaseProviderRW;
use reth_stages_api::{ExecInput, ExecOutput, StageError, UnwindInput, UnwindOutput};

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
