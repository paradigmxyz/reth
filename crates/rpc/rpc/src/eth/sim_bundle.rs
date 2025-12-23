//! `Eth` Sim bundle implementation and helpers.

use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumberOrTag;
use alloy_evm::{env::BlockEnvironment, overrides::apply_block_overrides};
use alloy_primitives::{keccak256, B256, U256};
use alloy_rpc_types_eth::BlockId;
use alloy_rpc_types_mev::{
    BundleItem, Inclusion, MevSendBundle, Privacy, RefundConfig, SimBundleLogs, SimBundleOverrides,
    SimBundleResponse, Validity,
};
use jsonrpsee::core::RpcResult;
use reth_evm::{ConfigureEvm, Evm};
use reth_primitives_traits::Recovered;
use reth_rpc_api::MevSimApiServer;
use reth_rpc_eth_api::{
    helpers::{block::LoadBlock, Call, EthTransactions},
    FromEthApiError, FromEvmError,
};
use reth_rpc_eth_types::{utils::recover_raw_transaction, EthApiError};
use reth_storage_api::ProviderTx;
use reth_tasks::pool::BlockingTaskGuard;
use reth_transaction_pool::{PoolPooledTx, PoolTransaction, TransactionPool};
use revm::{
    context::Block, context_interface::result::ResultAndState, DatabaseCommit, DatabaseRef,
};
use std::{collections::HashMap, sync::Arc, time::Duration};
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
const SBUNDLE_PAYOUT_MAX_COST: u64 = 30_000;

/// A flattened representation of a bundle item containing transaction and associated metadata.
#[derive(Clone, Debug)]
pub struct FlattenedBundleItem<T> {
    /// The signed transaction
    pub tx: Recovered<T>,
    /// Hash of the raw transaction bytes (keccak256 of original bytes)
    /// Used for matching logs with the original bundle structure
    pub raw_tx_hash: B256,
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
    /// Path from root bundle to this item, represented as indices at each nesting level.
    /// For example, `[0, 2]` means this item is at `bundle.bundle_body[0].bundle.bundle_body[2]`.
    /// An empty path means this item is at the root level.
    pub bundle_path: Vec<usize>,
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
}

impl<Eth> EthSimBundle<Eth>
where
    Eth: EthTransactions + LoadBlock + Call + 'static,
{
    /// Builds a hierarchical `SimBundleLogs` structure from a flat map of transaction logs,
    /// preserving the original nested bundle structure.
    ///
    /// This function iteratively traverses the original bundle structure using a stack
    /// and constructs `SimBundleLogs` entries that maintain the parent-child relationship.
    fn build_bundle_logs(
        bundle: &MevSendBundle,
        tx_logs_map: &HashMap<B256, Vec<alloy_rpc_types_eth::Log>>,
    ) -> Vec<SimBundleLogs> {
        // Stack item: (bundle, item_index, accumulated_logs)
        // We process bundles iteratively, building logs from leaves up
        let mut stack: Vec<(&MevSendBundle, usize, Vec<SimBundleLogs>)> = Vec::new();
        stack.push((bundle, 0, Vec::new()));

        // Final result - will be set when root bundle processing completes
        let mut final_result: Vec<SimBundleLogs> = Vec::new();

        while let Some((current_bundle, idx, mut current_logs)) = stack.pop() {
            let body = &current_bundle.bundle_body;

            if idx < body.len() {
                match &body[idx] {
                    BundleItem::Tx { tx, .. } => {
                        let tx_hash = keccak256(tx);
                        let tx_logs = tx_logs_map.get(&tx_hash).cloned();
                        current_logs.push(SimBundleLogs { tx_logs, bundle_logs: None });
                        // Continue with next item
                        stack.push((current_bundle, idx + 1, current_logs));
                    }
                    BundleItem::Bundle { bundle: nested_bundle } => {
                        // Save current state to resume after nested bundle is processed
                        stack.push((current_bundle, idx + 1, current_logs));
                        // Start processing nested bundle
                        stack.push((nested_bundle, 0, Vec::new()));
                    }
                    BundleItem::Hash { .. } => {
                        current_logs.push(SimBundleLogs { tx_logs: None, bundle_logs: None });
                        // Continue with next item
                        stack.push((current_bundle, idx + 1, current_logs));
                    }
                }
            } else {
                // Finished processing this bundle
                // Check if there's a parent waiting on the stack
                if let Some((parent_bundle, parent_idx, mut parent_logs)) = stack.pop() {
                    // Add this bundle's logs as nested bundle_logs in parent
                    parent_logs.push(SimBundleLogs { tx_logs: None, bundle_logs: Some(current_logs) });
                    // Put parent back on stack to continue processing
                    stack.push((parent_bundle, parent_idx, parent_logs));
                } else {
                    // This is the root bundle, we're done
                    final_result = current_logs;
                }
            }
        }

        final_result
    }

    /// Flattens a potentially nested bundle into a list of individual transactions in a
    /// `FlattenedBundleItem` with their associated metadata. This handles recursive bundle
    /// processing up to `MAX_NESTED_BUNDLE_DEPTH` and `MAX_BUNDLE_BODY_SIZE`, preserving
    /// inclusion, validity and privacy settings from parent bundles.
    fn parse_and_flatten_bundle(
        &self,
        request: &MevSendBundle,
    ) -> Result<Vec<FlattenedBundleItem<ProviderTx<Eth::Provider>>>, EthApiError> {
        let mut items = Vec::new();

        // Stack for processing bundles: (bundle, index, depth, parent_path)
        let mut stack: Vec<(&MevSendBundle, usize, usize, Vec<usize>)> = Vec::new();

        // Start with initial bundle, index 0, depth 1, and empty path
        stack.push((request, 0, 1, Vec::new()));

        while let Some((current_bundle, mut idx, depth, parent_path)) = stack.pop() {
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
                    BundleItem::Tx { tx: raw_tx, can_revert } => {
                        // Compute hash of raw transaction bytes for consistent log matching
                        let raw_tx_hash = keccak256(raw_tx);

                        let recovered_tx =
                            recover_raw_transaction::<PoolPooledTx<Eth::Pool>>(raw_tx)?;
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

                        // Build the full path for this item
                        let mut bundle_path = parent_path.clone();
                        bundle_path.push(idx);

                        // Create FlattenedBundleItem with current inclusion, validity, and privacy
                        let flattened_item = FlattenedBundleItem {
                            tx,
                            raw_tx_hash,
                            can_revert: *can_revert,
                            inclusion: inclusion.clone(),
                            validity: validity.clone(),
                            privacy: privacy.clone(),
                            refund_percent,
                            refund_configs,
                            bundle_path,
                        };

                        // Add to items
                        items.push(flattened_item);

                        idx += 1;
                    }
                    BundleItem::Bundle { bundle } => {
                        // Build path for nested bundle
                        let mut nested_path = parent_path.clone();
                        nested_path.push(idx);

                        // Push the current bundle and next index onto the stack to resume later
                        stack.push((current_bundle, idx + 1, depth, parent_path.clone()));

                        // process the nested bundle next
                        stack.push((bundle, 0, depth + 1, nested_path));
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
        let SimBundleOverrides { parent_block, block_overrides, .. } = overrides;

        // Parse and validate bundle
        // Also, flatten the bundle here so that its easier to process
        let flattened_bundle = self.parse_and_flatten_bundle(&request)?;

        let block_id = parent_block.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));
        let (mut evm_env, current_block_id) = self.eth_api().evm_env_at(block_id).await?;
        let current_block = self.eth_api().recovered_block(current_block_id).await?;
        let current_block = current_block.ok_or(EthApiError::HeaderNotFound(block_id))?;

        let eth_api = self.inner.eth_api.clone();

        let sim_response = self
            .inner
            .eth_api
            .spawn_with_state_at_block(current_block_id, move |_, mut db| {
                // Setup environment
                let current_block_number = current_block.number();
                let coinbase = evm_env.block_env.beneficiary();
                let basefee = evm_env.block_env.basefee();

                // apply overrides
                apply_block_overrides(block_overrides, &mut db, evm_env.block_env.inner_mut());

                let initial_coinbase_balance = DatabaseRef::basic_ref(&db, coinbase)
                    .map_err(EthApiError::from_eth_err)?
                    .map(|acc| acc.balance)
                    .unwrap_or_default();

                let mut coinbase_balance_before_tx = initial_coinbase_balance;
                let mut total_gas_used = 0;
                let mut total_profit = U256::ZERO;
                let mut refundable_value = U256::ZERO;
                // Map from transaction hash to logs for building hierarchical log structure
                let mut tx_logs_map: HashMap<B256, Vec<alloy_rpc_types_eth::Log>> = HashMap::new();

                let mut evm = eth_api.evm_config().evm_with_env(db, evm_env);
                let mut log_index = 0;

                for (tx_index, item) in flattened_bundle.iter().enumerate() {
                    // Check inclusion constraints
                    let block_number = item.inclusion.block_number();
                    let max_block_number =
                        item.inclusion.max_block_number().unwrap_or(block_number);

                    if current_block_number < block_number || current_block_number > max_block_number {
                        return Err(EthApiError::InvalidParams(
                            EthSimBundleError::InvalidInclusion.to_string(),
                        )
                        .into());
                    }

                    let ResultAndState { result, state } = evm
                        .transact(eth_api.evm_config().tx_env(&item.tx))
                        .map_err(Eth::Error::from_evm_err)?;

                    if !result.is_success() && !item.can_revert {
                        return Err(EthApiError::InvalidParams(
                            EthSimBundleError::BundleTransactionFailed.to_string(),
                        )
                        .into());
                    }

                    let gas_used = result.gas_used();
                    total_gas_used += gas_used;

                    // coinbase is always present in the result state
                    let coinbase_balance_after_tx =
                        state.get(&coinbase).map(|acc| acc.info.balance).unwrap_or_default();

                    let coinbase_diff =
                        coinbase_balance_after_tx.saturating_sub(coinbase_balance_before_tx);
                    total_profit += coinbase_diff;

                    // Add to refundable value if this tx does not have a refund percent
                    if item.refund_percent.is_none() {
                        refundable_value += coinbase_diff;
                    }

                    // Update coinbase balance before next tx
                    coinbase_balance_before_tx = coinbase_balance_after_tx;

                    // Collect logs if requested - store in map keyed by raw transaction hash
                    // Use raw_tx_hash for consistent matching with build_bundle_logs
                    if logs {
                        let tx_hash = item.raw_tx_hash;
                        let tx_logs: Vec<alloy_rpc_types_eth::Log> = result
                            .into_logs()
                            .into_iter()
                            .map(|inner| {
                                let full_log = alloy_rpc_types_eth::Log {
                                    inner,
                                    block_hash: None,
                                    block_number: None,
                                    block_timestamp: None,
                                    transaction_hash: Some(tx_hash),
                                    transaction_index: Some(tx_index as u64),
                                    log_index: Some(log_index),
                                    removed: false,
                                };
                                log_index += 1;
                                full_log
                            })
                            .collect();
                        tx_logs_map.insert(tx_hash, tx_logs);
                    }

                    // Apply state changes
                    evm.db_mut().commit(state);
                }

                // Build hierarchical log structure that preserves the original bundle nesting
                let body_logs =
                    if logs { Self::build_bundle_logs(&request, &tx_logs_map) } else { vec![] };

                // After processing all transactions, process refunds
                // Store the original refundable value to calculate all payouts correctly
                let original_refundable_value = refundable_value;
                for item in &flattened_bundle {
                    if let Some(refund_percent) = item.refund_percent {
                        // Get refund configurations
                        let refund_configs = item.refund_configs.clone().unwrap_or_else(|| {
                            vec![RefundConfig { address: item.tx.signer(), percent: 100 }]
                        });

                        // Calculate payout transaction fee
                        let payout_tx_fee = U256::from(basefee) * U256::from(SBUNDLE_PAYOUT_MAX_COST) * U256::from(refund_configs.len() as u64);

                        // Add gas used for payout transactions
                        total_gas_used += SBUNDLE_PAYOUT_MAX_COST * refund_configs.len() as u64;

                        // Calculate allocated refundable value (payout value) based on ORIGINAL
                        // refundable value This ensures all refund_percent
                        // values are calculated from the same base
                        let payout_value = original_refundable_value * U256::from(refund_percent) / U256::from(100);

                        if payout_tx_fee > payout_value {
                            return Err(EthApiError::InvalidParams(
                                EthSimBundleError::NegativeProfit.to_string(),
                            )
                            .into());
                        }

                        // Subtract payout value from total profit
                        total_profit = total_profit.checked_sub(payout_value).ok_or(
                            EthApiError::InvalidParams(
                                EthSimBundleError::NegativeProfit.to_string(),
                            ),
                        )?;

                        // Adjust refundable value
                        refundable_value = refundable_value.checked_sub(payout_value).ok_or(
                            EthApiError::InvalidParams(
                                EthSimBundleError::NegativeProfit.to_string(),
                            ),
                        )?;
                    }
                }

                // Calculate mev gas price
                let mev_gas_price = if total_gas_used != 0 {
                    total_profit / U256::from(total_gas_used)
                } else {
                    U256::ZERO
                };

                Ok(SimBundleResponse {
                    success: true,
                    state_block: current_block_number,
                    error: None,
                    logs: Some(body_logs),
                    gas_used: total_gas_used,
                    mev_gas_price,
                    profit: total_profit,
                    refundable_value,
                    exec_error: None,
                    revert: None,
                })
            })
            .await?;

        Ok(sim_response)
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
    use alloy_rpc_types_eth::Log;
    use alloy_rpc_types_mev::{Inclusion, ProtocolVersion};

    fn build_bundle_logs_test(
        bundle: &MevSendBundle,
        tx_logs_map: &HashMap<B256, Vec<Log>>,
    ) -> Vec<SimBundleLogs> {
        let mut stack: Vec<(&MevSendBundle, usize, Vec<SimBundleLogs>)> = Vec::new();
        stack.push((bundle, 0, Vec::new()));
        let mut final_result: Vec<SimBundleLogs> = Vec::new();

        while let Some((current_bundle, idx, mut current_logs)) = stack.pop() {
            let body = &current_bundle.bundle_body;
            if idx < body.len() {
                match &body[idx] {
                    BundleItem::Tx { tx, .. } => {
                        let tx_hash = keccak256(tx);
                        let tx_logs = tx_logs_map.get(&tx_hash).cloned();
                        current_logs.push(SimBundleLogs { tx_logs, bundle_logs: None });
                        stack.push((current_bundle, idx + 1, current_logs));
                    }
                    BundleItem::Bundle { bundle: nested_bundle } => {
                        stack.push((current_bundle, idx + 1, current_logs));
                        stack.push((nested_bundle, 0, Vec::new()));
                    }
                    BundleItem::Hash { .. } => {
                        current_logs.push(SimBundleLogs { tx_logs: None, bundle_logs: None });
                        stack.push((current_bundle, idx + 1, current_logs));
                    }
                }
            } else {
                if let Some((parent_bundle, parent_idx, mut parent_logs)) = stack.pop() {
                    parent_logs.push(SimBundleLogs { tx_logs: None, bundle_logs: Some(current_logs) });
                    stack.push((parent_bundle, parent_idx, parent_logs));
                } else {
                    final_result = current_logs;
                }
            }
        }
        final_result
    }

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

    #[test]
    fn test_build_bundle_logs_single_tx() {
        let tx_bytes = Bytes::from(vec![0x01, 0x02, 0x03]);
        let tx_hash = keccak256(&tx_bytes);
        let bundle = create_test_bundle(vec![tx_bytes]);

        let mut tx_logs_map = HashMap::new();
        tx_logs_map.insert(tx_hash, vec![Log::default()]);

        let result = build_bundle_logs_test(&bundle, &tx_logs_map);

        assert_eq!(result.len(), 1);
        assert!(result[0].tx_logs.is_some());
        assert!(result[0].bundle_logs.is_none());
        assert_eq!(result[0].tx_logs.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_build_bundle_logs_nested_bundle() {
        let outer_tx = Bytes::from(vec![0x01, 0x02, 0x03]);
        let inner_tx1 = Bytes::from(vec![0x04, 0x05, 0x06]);
        let inner_tx2 = Bytes::from(vec![0x07, 0x08, 0x09]);

        let outer_hash = keccak256(&outer_tx);
        let inner_hash1 = keccak256(&inner_tx1);
        let inner_hash2 = keccak256(&inner_tx2);

        let bundle = create_nested_bundle(outer_tx, vec![inner_tx1, inner_tx2]);

        let mut tx_logs_map = HashMap::new();
        tx_logs_map.insert(outer_hash, vec![Log::default()]);
        tx_logs_map.insert(inner_hash1, vec![Log::default()]);
        tx_logs_map.insert(inner_hash2, vec![Log::default(), Log::default()]);

        let result = build_bundle_logs_test(&bundle, &tx_logs_map);

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
    fn test_build_bundle_logs_missing_logs() {
        let tx_bytes = Bytes::from(vec![0x01, 0x02, 0x03]);
        let bundle = create_test_bundle(vec![tx_bytes]);
        let tx_logs_map = HashMap::new();

        let result = build_bundle_logs_test(&bundle, &tx_logs_map);

        assert_eq!(result.len(), 1);
        assert!(result[0].tx_logs.is_none());
        assert!(result[0].bundle_logs.is_none());
    }

    #[test]
    fn test_build_bundle_logs_deeply_nested() {
        let tx1 = Bytes::from(vec![0x01]);
        let tx2 = Bytes::from(vec![0x02]);
        let tx3 = Bytes::from(vec![0x03]);

        let hash1 = keccak256(&tx1);
        let hash2 = keccak256(&tx2);
        let hash3 = keccak256(&tx3);

        let bundle_c = create_test_bundle(vec![tx3.clone()]);
        let bundle_b = MevSendBundle {
            bundle_body: vec![
                BundleItem::Tx { tx: tx2.clone(), can_revert: false },
                BundleItem::Bundle { bundle: bundle_c },
            ],
            inclusion: Inclusion { block: 1, max_block: None },
            validity: None,
            privacy: None,
            protocol_version: ProtocolVersion::V0_1,
        };
        let bundle_a = MevSendBundle {
            bundle_body: vec![
                BundleItem::Tx { tx: tx1.clone(), can_revert: false },
                BundleItem::Bundle { bundle: bundle_b },
            ],
            inclusion: Inclusion { block: 1, max_block: None },
            validity: None,
            privacy: None,
            protocol_version: ProtocolVersion::V0_1,
        };

        let mut tx_logs_map = HashMap::new();
        tx_logs_map.insert(hash1, vec![Log::default()]);
        tx_logs_map.insert(hash2, vec![Log::default()]);
        tx_logs_map.insert(hash3, vec![Log::default()]);

        let result = build_bundle_logs_test(&bundle_a, &tx_logs_map);

        assert_eq!(result.len(), 2);
        assert!(result[0].tx_logs.is_some());
        assert!(result[1].bundle_logs.is_some());

        let bundle_b_logs = result[1].bundle_logs.as_ref().unwrap();
        assert_eq!(bundle_b_logs.len(), 2);
        assert!(bundle_b_logs[0].tx_logs.is_some());
        assert!(bundle_b_logs[1].bundle_logs.is_some());

        let bundle_c_logs = bundle_b_logs[1].bundle_logs.as_ref().unwrap();
        assert_eq!(bundle_c_logs.len(), 1);
        assert!(bundle_c_logs[0].tx_logs.is_some());
    }
}
