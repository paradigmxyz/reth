use crate::metrics::{Operation, Transaction, TransactionMode, TransactionOutcome};
use metrics::Histogram;
use reth_metrics::{metrics::Counter, Metrics};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

#[derive(Debug, Default)]
pub(crate) struct Metrics {
    transactions: HashMap<u64, Transaction>,
    transaction_metrics: HashMap<(TransactionMode, TransactionOutcome), TransactionMetrics>,

    operation_metrics: HashMap<Operation, OperationMetrics>,
}

impl Metrics {
    pub(crate) fn record_open_transaction(&mut self, txn_id: u64, mode: TransactionMode) {
        self.transactions.insert(txn_id, Transaction { begin: Instant::now(), mode });
    }

    pub(crate) fn record_close_transaction(
        &mut self,
        txn_id: u64,
        outcome: TransactionOutcome,
        commit_duration: Duration,
    ) {
        if let Some(transaction) = self.transactions.remove(&txn_id) {
            let metrics =
                self.transaction_metrics.entry((transaction.mode, outcome)).or_insert_with(|| {
                    TransactionMetrics::new_with_labels(&[
                        ("mode", transaction.mode.to_string()),
                        ("outcome", outcome.to_string()),
                    ])
                });
            metrics.open_duration.record(transaction.begin.elapsed());
            metrics.commit_duration.record(commit_duration);
        }
    }

    pub(crate) fn record_operation(&mut self, operation: Operation, duration: Duration) {
        let metrics = self.operation_metrics.entry(operation).or_insert_with(|| {
            OperationMetrics::new_with_labels(&[("operation", operation.to_string())])
        });

        metrics.duration.record(duration);
        metrics.called.increment(1);
    }
}

#[derive(Metrics)]
#[metrics(scope = "database.transaction")]
struct TransactionMetrics {
    /// open duration
    pub(crate) open_duration: Histogram,
    /// commit duration
    pub(crate) commit_duration: Histogram,
}

#[derive(Metrics)]
#[metrics(scope = "database.operation")]
struct OperationMetrics {
    /// called
    pub(crate) called: Counter,
    /// duration
    pub(crate) duration: Histogram,
}
