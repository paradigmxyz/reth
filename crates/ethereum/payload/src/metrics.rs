//! Payload builder metrics

use reth_metrics::metrics;

/// Record inclusion list transaction included when building a block
pub(crate) fn record_inclusion_list_transaction_included() {
    metrics::counter!("ef_execution_block_building_inclusion_list_transactions_included_total")
        .increment(1);
}

/// Record inclusion list transaction excluded when building a block with reason
pub(crate) fn record_inclusion_list_transaction_excluded(reason: &'static str) {
    metrics::counter!(
        "ef_execution_block_building_inclusion_list_transactions_excluded_total",
        "reason" => reason
    )
    .increment(1);
}
