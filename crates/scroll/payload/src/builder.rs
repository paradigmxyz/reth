use core::fmt::Debug;
use reth_basic_payload_builder::{
    BuildArguments, BuildOutcome, HeaderForPayload, PayloadBuilder, PayloadConfig,
};
use reth_payload_primitives::PayloadBuilderError;
use reth_payload_util::{BestPayloadTransactions, PayloadTransactions};
use reth_scroll_engine_primitives::{ScrollBuiltPayload, ScrollPayloadBuilderAttributes};
use reth_transaction_pool::{BestTransactionsAttributes, PoolTransaction, TransactionPool};

/// A type that implements [`PayloadBuilder`] by building empty payloads.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct ScrollEmptyPayloadBuilder;

impl PayloadBuilder for ScrollEmptyPayloadBuilder {
    type Attributes = ScrollPayloadBuilderAttributes;
    type BuiltPayload = ScrollBuiltPayload;

    fn try_build(
        &self,
        _args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        // we can't currently actually build a payload, so we mark the outcome as cancelled.
        Ok(BuildOutcome::Cancelled)
    }

    fn build_empty_payload(
        &self,
        _config: PayloadConfig<Self::Attributes, HeaderForPayload<Self::BuiltPayload>>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        Ok(ScrollBuiltPayload::default())
    }
}

/// A type that returns the [`PayloadTransactions`] that should be included in the pool.
pub trait ScrollPayloadTransactions<Transaction>: Clone + Send + Sync + Unpin + 'static {
    /// Returns an iterator that yields the transaction in the order they should get included in the
    /// new payload.
    fn best_transactions<Pool: TransactionPool<Transaction = Transaction>>(
        &self,
        pool: Pool,
        attr: BestTransactionsAttributes,
    ) -> impl PayloadTransactions<Transaction = Transaction>;
}

impl<T: PoolTransaction> ScrollPayloadTransactions<T> for () {
    fn best_transactions<Pool: TransactionPool<Transaction = T>>(
        &self,
        pool: Pool,
        attr: BestTransactionsAttributes,
    ) -> impl PayloadTransactions<Transaction = T> {
        BestPayloadTransactions::new(pool.best_transactions_with_attributes(attr))
    }
}
