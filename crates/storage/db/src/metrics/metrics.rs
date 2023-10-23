use crate::metrics::{Operation, Transaction, TransactionMode, TransactionOutcome};
use metrics::{Gauge, Histogram};
use reth_metrics::{metrics::Counter, Metrics};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

#[derive(Metrics)]
#[metrics(scope = "database")]
pub(crate) struct Metrics {
    /// Total number of currently open database transactions
    open_transactions_total: Gauge,

    #[metric(skip)]
    transactions: HashMap<u64, Transaction>,
    #[metric(skip)]
    transaction_metrics: HashMap<(TransactionMode, TransactionOutcome), TransactionMetrics>,
    #[metric(skip)]
    operation_metrics: HashMap<Operation, OperationMetrics>,
}

impl Metrics {
    pub(crate) fn record_open_transaction(&mut self, txn_id: u64, mode: TransactionMode) {
        self.transactions.insert(txn_id, Transaction { begin: Instant::now(), mode });
        self.open_transactions_total.set(self.transactions.len() as f64)
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
            metrics.open_duration_seconds.record(transaction.begin.elapsed());
            metrics.commit_duration_seconds.record(commit_duration);

            self.open_transactions_total.set(self.transactions.len() as f64)
        }
    }

    pub(crate) fn record_operation(&mut self, operation: Operation, duration: Duration) {
        let metrics = self.operation_metrics.entry(operation).or_insert_with(|| {
            OperationMetrics::new_with_labels(&[("operation", operation.to_string())])
        });

        metrics.calls_total.increment(1);
        metrics.duration_seconds.record(duration);
    }
}

#[derive(Metrics)]
#[metrics(scope = "database.transaction")]
struct TransactionMetrics {
    /// The time a database transaction has been open
    pub(crate) open_duration_seconds: Histogram,
    /// Database transaction commit duration
    pub(crate) commit_duration_seconds: Histogram,
}

#[derive(Metrics)]
#[metrics(scope = "database.operation")]
struct OperationMetrics {
    /// Total number of database operations made
    pub(crate) calls_total: Counter,
    /// Database operation duration
    pub(crate) duration_seconds: Histogram,
}
