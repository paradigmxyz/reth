//! `Eth` Sim bundle implementation and helpers.

use alloy_consensus::transaction::Recovered;
#[cfg(test)]
use alloy_rpc_types_eth::Log;
#[cfg(test)]
use alloy_rpc_types_mev::SimBundleLogs;
use alloy_rpc_types_mev::{
    BundleItem, Inclusion, MevSendBundle, Privacy, RefundConfig, SimBundleOverrides,
    SimBundleResponse, Validity,
};
use jsonrpsee::core::RpcResult;
use reth_rpc_api::MevSimApiServer;
use reth_rpc_eth_api::helpers::{block::LoadBlock, Call, EthTransactions};
use reth_rpc_eth_types::{utils::recover_raw_transaction, EthApiError};
use reth_storage_api::ProviderTx;
use reth_tasks::pool::BlockingTaskGuard;
use reth_transaction_pool::{PoolPooledTx, PoolTransaction, TransactionPool};
use std::{sync::Arc, time::Duration};
use tracing::trace;

/// Maximum bundle depth
const MAX_NESTED_BUNDLE_DEPTH: usize = 5;

/// Maximum body size
const MAX_BUNDLE_BODY_SIZE: usize = 50;

/// Default simulation timeout
const DEFAULT_SIM_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum simulation timeout
const MAX_SIM_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum payout cost
#[expect(dead_code)]
const SBUNDLE_PAYOUT_MAX_COST: u64 = 30_000;

/// A flattened representation of a bundle item containing transaction and associated metadata.
#[derive(Clone, Debug)]
pub struct FlattenedBundleItem<T> {
    /// The signed transaction
    pub tx: Recovered<T>,
    /// Whether the transaction is allowed to revert
    pub can_revert: bool,
    /// Item-level inclusion constraints
    pub inclusion: Inclusion,
    /// Optional validity constraints for the bundle item
    pub validity: Option<Validity>,
    /// Optional privacy settings for the bundle item
    pub privacy: Option<Privacy>,
    /// Optional refund percent for the bundle item
    pub refund_percent: Option<u64>,
    /// Optional refund configs for the bundle item
    pub refund_configs: Option<Vec<RefundConfig>>,
}

/// `Eth` sim bundle implementation.
pub struct EthSimBundle<Eth> {
    /// All nested fields bundled together.
    inner: Arc<EthSimBundleInner<Eth>>,
}

impl<Eth> EthSimBundle<Eth> {
    /// Create a new `EthSimBundle` instance.
    pub fn new(eth_api: Eth, blocking_task_guard: BlockingTaskGuard) -> Self {
        Self { inner: Arc::new(EthSimBundleInner { eth_api, blocking_task_guard }) }
    }

    /// Access the underlying `Eth` API.
    pub fn eth_api(&self) -> &Eth {
        &self.inner.eth_api
    }

    /// Builds a hierarchical `SimBundleLogs` structure from flattened transaction logs.
    #[cfg(test)]
    fn build_bundle_logs(
        bundle: &MevSendBundle,
        flat_logs: &[Vec<Log>],
    ) -> Result<Vec<SimBundleLogs>, EthApiError> {
        struct BundleFrame<'a> {
            bundle: &'a MevSendBundle,
            next_idx: usize,
            logs: Vec<SimBundleLogs>,
        }

        let mut stack = vec![BundleFrame { bundle, next_idx: 0, logs: Vec::new() }];
        let mut flat_log_idx = 0;
        let mut root_logs = None;

        while let Some(mut frame) = stack.pop() {
            if frame.next_idx == frame.bundle.bundle_body.len() {
                if let Some(parent) = stack.last_mut() {
                    parent
                        .logs
                        .push(SimBundleLogs { tx_logs: None, bundle_logs: Some(frame.logs) });
                } else {
                    root_logs = Some(frame.logs);
                }

                continue;
            }

            match &frame.bundle.bundle_body[frame.next_idx] {
                BundleItem::Tx { .. } => {
                    let tx_logs = flat_logs.get(flat_log_idx).cloned().ok_or_else(|| {
                        EthApiError::InvalidParams(EthSimBundleError::UnmatchedBundle.to_string())
                    })?;

                    frame.logs.push(SimBundleLogs { tx_logs: Some(tx_logs), bundle_logs: None });
                    frame.next_idx += 1;
                    flat_log_idx += 1;
                    stack.push(frame);
                }
                BundleItem::Bundle { bundle } => {
                    frame.next_idx += 1;
                    stack.push(frame);
                    stack.push(BundleFrame { bundle, next_idx: 0, logs: Vec::new() });
                }
                BundleItem::Hash { .. } => {
                    return Err(EthApiError::InvalidParams(
                        EthSimBundleError::InvalidBundle.to_string(),
                    ));
                }
            }
        }

        if flat_log_idx != flat_logs.len() {
            return Err(EthApiError::InvalidParams(EthSimBundleError::UnmatchedBundle.to_string()));
        }

        root_logs.ok_or_else(|| {
            EthApiError::InvalidParams(EthSimBundleError::UnmatchedBundle.to_string())
        })
    }
}

impl<Eth> EthSimBundle<Eth>
where
    Eth: EthTransactions + LoadBlock + Call + 'static,
{
    /// Flattens a potentially nested bundle into a list of individual transactions in a
    /// `FlattenedBundleItem` with their associated metadata. This handles recursive bundle
    /// processing up to `MAX_NESTED_BUNDLE_DEPTH` and `MAX_BUNDLE_BODY_SIZE`, preserving
    /// inclusion, validity and privacy settings from parent bundles.
    #[expect(dead_code)]
    fn parse_and_flatten_bundle(
        &self,
        request: &MevSendBundle,
    ) -> Result<Vec<FlattenedBundleItem<ProviderTx<Eth::Provider>>>, EthApiError> {
        let mut items = Vec::new();

        // Stack for processing bundles
        let mut stack = Vec::new();

        // Start with initial bundle, index 0, and depth 1
        stack.push((request, 0, 1));

        while let Some((current_bundle, mut idx, depth)) = stack.pop() {
            // Check max depth
            if depth > MAX_NESTED_BUNDLE_DEPTH {
                return Err(EthApiError::InvalidParams(EthSimBundleError::MaxDepth.to_string()));
            }

            // Determine inclusion, validity, and privacy
            let inclusion = &current_bundle.inclusion;
            let validity = &current_bundle.validity;
            let privacy = &current_bundle.privacy;

            // Validate inclusion parameters
            let block_number = inclusion.block_number();
            let max_block_number = inclusion.max_block_number().unwrap_or(block_number);

            if max_block_number < block_number || block_number == 0 {
                return Err(EthApiError::InvalidParams(
                    EthSimBundleError::InvalidInclusion.to_string(),
                ));
            }

            // Validate bundle body size
            if current_bundle.bundle_body.len() > MAX_BUNDLE_BODY_SIZE {
                return Err(EthApiError::InvalidParams(
                    EthSimBundleError::BundleTooLarge.to_string(),
                ));
            }

            // Validate validity and refund config
            if let Some(validity) = &current_bundle.validity {
                // Validate refund entries
                if let Some(refunds) = &validity.refund {
                    let mut total_percent = 0;
                    for refund in refunds {
                        if refund.body_idx as usize >= current_bundle.bundle_body.len() {
                            return Err(EthApiError::InvalidParams(
                                EthSimBundleError::InvalidValidity.to_string(),
                            ));
                        }
                        if 100 - total_percent < refund.percent {
                            return Err(EthApiError::InvalidParams(
                                EthSimBundleError::InvalidValidity.to_string(),
                            ));
                        }
                        total_percent += refund.percent;
                    }
                }

                // Validate refund configs
                if let Some(refund_configs) = &validity.refund_config {
                    let mut total_percent = 0;
                    for refund_config in refund_configs {
                        if 100 - total_percent < refund_config.percent {
                            return Err(EthApiError::InvalidParams(
                                EthSimBundleError::InvalidValidity.to_string(),
                            ));
                        }
                        total_percent += refund_config.percent;
                    }
                }
            }

            let body = &current_bundle.bundle_body;

            // Process items in the current bundle
            while idx < body.len() {
                match &body[idx] {
                    BundleItem::Tx { tx, can_revert } => {
                        let recovered_tx = recover_raw_transaction::<PoolPooledTx<Eth::Pool>>(tx)?;
                        let tx = recovered_tx.map(
                            <Eth::Pool as TransactionPool>::Transaction::pooled_into_consensus,
                        );

                        let refund_percent =
                            validity.as_ref().and_then(|v| v.refund.as_ref()).and_then(|refunds| {
                                refunds.iter().find_map(|refund| {
                                    (refund.body_idx as usize == idx).then_some(refund.percent)
                                })
                            });
                        let refund_configs =
                            validity.as_ref().and_then(|v| v.refund_config.clone());

                        // Create FlattenedBundleItem with current inclusion, validity, and privacy
                        let flattened_item = FlattenedBundleItem {
                            tx,
                            can_revert: *can_revert,
                            inclusion: inclusion.clone(),
                            validity: validity.clone(),
                            privacy: privacy.clone(),
                            refund_percent,
                            refund_configs,
                        };

                        items.push(flattened_item);
                        idx += 1;
                    }
                    BundleItem::Bundle { bundle } => {
                        // Push the current bundle and next index onto the stack to resume later
                        stack.push((current_bundle, idx + 1, depth));

                        // process the nested bundle next
                        stack.push((bundle, 0, depth + 1));
                        break;
                    }
                    BundleItem::Hash { hash: _ } => {
                        // Hash-only items are not allowed
                        return Err(EthApiError::InvalidParams(
                            EthSimBundleError::InvalidBundle.to_string(),
                        ));
                    }
                }
            }
        }

        Ok(items)
    }

    async fn sim_bundle_inner(
        &self,
        request: MevSendBundle,
        overrides: SimBundleOverrides,
        logs: bool,
    ) -> Result<SimBundleResponse, Eth::Error> {
        let _ = (request, overrides, logs);
        Err(EthApiError::Unsupported("mev_simBundle is unsupported by the evm2 execution path")
            .into())
    }
}

#[async_trait::async_trait]
impl<Eth> MevSimApiServer for EthSimBundle<Eth>
where
    Eth: EthTransactions + LoadBlock + Call + 'static,
{
    async fn sim_bundle(
        &self,
        request: MevSendBundle,
        overrides: SimBundleOverrides,
    ) -> RpcResult<SimBundleResponse> {
        trace!("mev_simBundle called, request: {:?}, overrides: {:?}", request, overrides);

        let override_timeout = overrides.timeout;

        let timeout = override_timeout
            .map(Duration::from_secs)
            .map(|d| d.min(MAX_SIM_TIMEOUT))
            .unwrap_or(DEFAULT_SIM_TIMEOUT);

        let bundle_res =
            tokio::time::timeout(timeout, Self::sim_bundle_inner(self, request, overrides, true))
                .await
                .map_err(|_| {
                    EthApiError::InvalidParams(EthSimBundleError::BundleTimeout.to_string())
                })?;

        bundle_res.map_err(Into::into)
    }
}

/// Container type for `EthSimBundle` internals
#[derive(Debug)]
struct EthSimBundleInner<Eth> {
    /// Access to commonly used code of the `eth` namespace
    eth_api: Eth,
    // restrict the number of concurrent tracing calls.
    #[expect(dead_code)]
    blocking_task_guard: BlockingTaskGuard,
}

impl<Eth> std::fmt::Debug for EthSimBundle<Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthSimBundle").finish_non_exhaustive()
    }
}

impl<Eth> Clone for EthSimBundle<Eth> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

/// [`EthSimBundle`] specific errors.
#[derive(Debug, thiserror::Error)]
pub enum EthSimBundleError {
    /// Thrown when max depth is reached
    #[error("max depth reached")]
    MaxDepth,
    /// Thrown when a bundle is unmatched
    #[error("unmatched bundle")]
    UnmatchedBundle,
    /// Thrown when a bundle is too large
    #[error("bundle too large")]
    BundleTooLarge,
    /// Thrown when validity is invalid
    #[error("invalid validity")]
    InvalidValidity,
    /// Thrown when inclusion is invalid
    #[error("invalid inclusion")]
    InvalidInclusion,
    /// Thrown when a bundle is invalid
    #[error("invalid bundle")]
    InvalidBundle,
    /// Thrown when a bundle simulation times out
    #[error("bundle simulation timed out")]
    BundleTimeout,
    /// Thrown when a transaction is reverted in a bundle
    #[error("bundle transaction failed")]
    BundleTransactionFailed,
    /// Thrown when a bundle simulation returns negative profit
    #[error("bundle simulation returned negative profit")]
    NegativeProfit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;
    use alloy_rpc_types_mev::{Inclusion, ProtocolVersion};

    fn create_test_bundle(tx_bytes: Vec<Bytes>) -> MevSendBundle {
        let body: Vec<BundleItem> =
            tx_bytes.into_iter().map(|tx| BundleItem::Tx { tx, can_revert: false }).collect();
        MevSendBundle {
            bundle_body: body,
            inclusion: Inclusion { block: 1, max_block: None },
            validity: None,
            privacy: None,
            protocol_version: ProtocolVersion::V0_1,
        }
    }

    fn create_nested_bundle(outer_tx: Bytes, inner_txs: Vec<Bytes>) -> MevSendBundle {
        let inner_bundle = create_test_bundle(inner_txs);
        MevSendBundle {
            bundle_body: vec![
                BundleItem::Tx { tx: outer_tx, can_revert: false },
                BundleItem::Bundle { bundle: inner_bundle },
            ],
            inclusion: Inclusion { block: 1, max_block: None },
            validity: None,
            privacy: None,
            protocol_version: ProtocolVersion::V0_1,
        }
    }

    fn create_bundle_with_body(bundle_body: Vec<BundleItem>) -> MevSendBundle {
        MevSendBundle {
            bundle_body,
            inclusion: Inclusion { block: 1, max_block: None },
            validity: None,
            privacy: None,
            protocol_version: ProtocolVersion::V0_1,
        }
    }

    fn create_bundle_logs(log_counts: &[usize]) -> Vec<Vec<Log>> {
        log_counts.iter().map(|count| vec![Log::default(); *count]).collect()
    }

    fn assert_unmatched_bundle(result: Result<Vec<SimBundleLogs>, EthApiError>) {
        assert!(matches!(
            result,
            Err(EthApiError::InvalidParams(ref message))
                if message == &EthSimBundleError::UnmatchedBundle.to_string()
        ));
    }

    #[test]
    fn test_build_bundle_logs_single_tx() {
        let bundle = create_test_bundle(vec![Bytes::from(vec![0x01, 0x02, 0x03])]);
        let result =
            EthSimBundle::<()>::build_bundle_logs(&bundle, &create_bundle_logs(&[1])).unwrap();

        assert_eq!(result.len(), 1);
        assert!(result[0].tx_logs.is_some());
        assert!(result[0].bundle_logs.is_none());
        assert_eq!(result[0].tx_logs.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_build_bundle_logs_empty_bundle() {
        let bundle = create_test_bundle(vec![]);
        let result = EthSimBundle::<()>::build_bundle_logs(&bundle, &[]).unwrap();

        assert!(result.is_empty());
    }

    #[test]
    fn test_build_bundle_logs_nested_bundle() {
        let outer_tx = Bytes::from(vec![0x01, 0x02, 0x03]);
        let inner_tx1 = Bytes::from(vec![0x04, 0x05, 0x06]);
        let inner_tx2 = Bytes::from(vec![0x07, 0x08, 0x09]);
        let bundle = create_nested_bundle(outer_tx, vec![inner_tx1, inner_tx2]);
        let result =
            EthSimBundle::<()>::build_bundle_logs(&bundle, &create_bundle_logs(&[1, 1, 2]))
                .unwrap();

        assert_eq!(result.len(), 2);
        assert!(result[0].tx_logs.is_some());
        assert!(result[0].bundle_logs.is_none());
        assert_eq!(result[0].tx_logs.as_ref().unwrap().len(), 1);

        assert!(result[1].tx_logs.is_none());
        assert!(result[1].bundle_logs.is_some());

        let nested_logs = result[1].bundle_logs.as_ref().unwrap();
        assert_eq!(nested_logs.len(), 2);
        assert!(nested_logs[0].tx_logs.is_some());
        assert_eq!(nested_logs[0].tx_logs.as_ref().unwrap().len(), 1);
        assert!(nested_logs[1].tx_logs.is_some());
        assert_eq!(nested_logs[1].tx_logs.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_build_bundle_logs_duplicate_transactions_same_level() {
        let duplicate_tx = Bytes::from(vec![0x01, 0x02, 0x03]);
        let bundle = create_test_bundle(vec![duplicate_tx.clone(), duplicate_tx]);
        let result =
            EthSimBundle::<()>::build_bundle_logs(&bundle, &create_bundle_logs(&[1, 2])).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].tx_logs.as_ref().unwrap().len(), 1);
        assert_eq!(result[1].tx_logs.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_build_bundle_logs_duplicate_transactions_across_nested_bundles() {
        let duplicate_tx = Bytes::from(vec![0x01, 0x02, 0x03]);
        let bundle = create_nested_bundle(duplicate_tx.clone(), vec![duplicate_tx]);
        let result =
            EthSimBundle::<()>::build_bundle_logs(&bundle, &create_bundle_logs(&[1, 2])).unwrap();

        assert_eq!(result.len(), 2);
        assert!(result[1].bundle_logs.is_some());
        assert_eq!(result[0].tx_logs.as_ref().unwrap().len(), 1);

        let nested_logs = result[1].bundle_logs.as_ref().unwrap();
        assert_eq!(nested_logs.len(), 1);
        assert_eq!(nested_logs[0].tx_logs.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_build_bundle_logs_root_with_only_nested_bundles() {
        let first_nested = create_test_bundle(vec![Bytes::from(vec![0x01])]);
        let second_nested =
            create_test_bundle(vec![Bytes::from(vec![0x02]), Bytes::from(vec![0x03])]);
        let bundle = create_bundle_with_body(vec![
            BundleItem::Bundle { bundle: first_nested },
            BundleItem::Bundle { bundle: second_nested },
        ]);
        let result =
            EthSimBundle::<()>::build_bundle_logs(&bundle, &create_bundle_logs(&[1, 1, 2]))
                .unwrap();

        assert_eq!(result.len(), 2);
        assert!(result[0].tx_logs.is_none());
        assert!(result[1].tx_logs.is_none());

        let first_nested_logs = result[0].bundle_logs.as_ref().unwrap();
        assert_eq!(first_nested_logs.len(), 1);
        assert_eq!(first_nested_logs[0].tx_logs.as_ref().unwrap().len(), 1);

        let second_nested_logs = result[1].bundle_logs.as_ref().unwrap();
        assert_eq!(second_nested_logs.len(), 2);
        assert_eq!(second_nested_logs[0].tx_logs.as_ref().unwrap().len(), 1);
        assert_eq!(second_nested_logs[1].tx_logs.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_build_bundle_logs_deeply_nested_bundle() {
        let leaf_bundle = create_test_bundle(vec![Bytes::from(vec![0x03])]);
        let middle_bundle = create_bundle_with_body(vec![
            BundleItem::Tx { tx: Bytes::from(vec![0x02]), can_revert: false },
            BundleItem::Bundle { bundle: leaf_bundle },
        ]);
        let root_bundle = create_bundle_with_body(vec![
            BundleItem::Tx { tx: Bytes::from(vec![0x01]), can_revert: false },
            BundleItem::Bundle { bundle: middle_bundle },
        ]);
        let result =
            EthSimBundle::<()>::build_bundle_logs(&root_bundle, &create_bundle_logs(&[1, 2, 3]))
                .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].tx_logs.as_ref().unwrap().len(), 1);

        let middle_logs = result[1].bundle_logs.as_ref().unwrap();
        assert_eq!(middle_logs.len(), 2);
        assert_eq!(middle_logs[0].tx_logs.as_ref().unwrap().len(), 2);

        let leaf_logs = middle_logs[1].bundle_logs.as_ref().unwrap();
        assert_eq!(leaf_logs.len(), 1);
        assert_eq!(leaf_logs[0].tx_logs.as_ref().unwrap().len(), 3);
    }

    #[test]
    fn test_build_bundle_logs_mismatched_flat_logs() {
        let bundle = create_test_bundle(vec![Bytes::from(vec![0x01, 0x02, 0x03])]);

        assert_unmatched_bundle(EthSimBundle::<()>::build_bundle_logs(&bundle, &[]));
        assert_unmatched_bundle(EthSimBundle::<()>::build_bundle_logs(
            &bundle,
            &create_bundle_logs(&[1, 2]),
        ));
    }
}
