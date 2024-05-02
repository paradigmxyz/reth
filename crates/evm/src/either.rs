//! Helper type that represents one of two possible executor types


use revm_primitives::db::Database;
use reth_interfaces::provider::ProviderError;
use reth_primitives::PruneModes;
use crate::execute::BlockExecutorProvider;

/// A type that represents one of two possible executor factories.
#[derive(Debug, Clone)]
pub enum EitherExecutorProvider<A, B> {
    /// The first executor provider variant
    Left(A),
    /// The first executor provider variant
    Right(B),
}

impl<A, B, DB> BlockExecutorProvider<DB> for EitherExecutorProvider<A, B> {
    type Executor<DB: Database<Error=ProviderError>> = ();
    type BatchExecutor<DB: Database<Error=ProviderError>> = ();

    fn executor<DB>(&self, db: DB) -> Self::Executor<DB> where DB: Database<Error=ProviderError> {
        todo!()
    }

    fn batch_executor<DB>(&self, db: DB, prune_modes: PruneModes) -> Self::BatchExecutor<DB> where DB: Database<Error=ProviderError> {
        todo!()
    }
}