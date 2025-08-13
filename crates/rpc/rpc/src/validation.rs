use alloy_consensus::{
    BlobTransactionValidationError, BlockHeader, EnvKzgSettings, Transaction, TxReceipt,
};
use alloy_eips::{eip4844::kzg_to_versioned_hash, eip7685::RequestsOrHash};
use alloy_rpc_types_beacon::relay::{
    BidTrace, BuilderBlockValidationRequest, BuilderBlockValidationRequestV2,
};
use alloy_rpc_types_engine::{
    BlobsBundleV1, CancunPayloadFields, ExecutionData, ExecutionPayload, ExecutionPayloadSidecar,
    PraguePayloadFields,
};
use async_trait::async_trait;
use core::fmt;
use jsonrpsee::core::RpcResult;
use jsonrpsee_types::error::ErrorObject;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_consensus::{Consensus, FullConsensus};
use reth_engine_primitives::PayloadValidator;
use reth_errors::{BlockExecutionError, ConsensusError, ProviderError};
use reth_evm::execute::{BlockExecutorProvider, Executor};
use reth_execution_types::BlockExecutionOutput;
use reth_metrics::{Metrics, metrics, metrics::Gauge};
use reth_node_api::NewPayloadError;
use reth_primitives_traits::{
    BlockBody, GotExpected, NodePrimitives, RecoveredBlock, SealedBlock, SealedHeaderFor,
    constants::GAS_LIMIT_BOUND_DIVISOR,
};
use reth_revm::{cached::CachedReads, database::StateProviderDatabase};
use reth_rpc_api::{
    BlockSubmissionValidationApiServer, BuilderBlockValidationRequestV3,
    BuilderBlockValidationRequestV4, TransactionFilter,
};
use reth_rpc_server_types::result::{internal_rpc_err, invalid_params_rpc_err};
use reth_storage_api::{
    BlockReaderIdExt, HashedPostStateProvider, StateProviderFactory, StateRootProvider,
};
use reth_tasks::TaskSpawner;
use revm_primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::{RwLock, oneshot};
use tracing::warn;

/// Computes the ratio `proposer_paid` / (`proposer_paid` + `coinbase_delta`) in basis points
/// (x100 of percent). Returns `None` if the denominator is zero.
fn mev_ratio_bps(proposer_paid: U256, coinbase_delta: U256) -> Option<u128> {
    let denom = proposer_paid.saturating_add(coinbase_delta);
    // Proposer and builder are paid zero. Can't determine ratio.
    if denom.is_zero() {
        return None;
    }
    (proposer_paid.saturating_mul(U256::from(10_000u64)) / denom).try_into().ok()
}

/// Formats basis points (1/100th of a percent) as a percentage string with two decimals.
fn fmt_percent_2dp(bps: u128) -> String {
    let integer = bps / 100;
    let frac = bps % 100;
    format!("{integer}.{frac:02}%")
}

/// The type that implements the `validation` rpc namespace trait
#[derive(Clone, Debug, derive_more::Deref)]
pub struct ValidationApi<Provider, E: BlockExecutorProvider> {
    #[deref]
    inner: Arc<ValidationApiInner<Provider, E>>,
}

impl<Provider, E> ValidationApi<Provider, E>
where
    E: BlockExecutorProvider,
{
    /// Create a new instance of the [`ValidationApi`]
    pub fn new(
        provider: Provider,
        consensus: Arc<dyn FullConsensus<E::Primitives, Error = ConsensusError>>,
        executor_provider: E,
        config: ValidationApiConfig,
        task_spawner: Box<dyn TaskSpawner>,
        payload_validator: Arc<
            dyn PayloadValidator<
                    Block = <E::Primitives as NodePrimitives>::Block,
                    ExecutionData = ExecutionData,
                >,
        >,
    ) -> Self {
        let ValidationApiConfig { disallow, validation_window } = config;

        let inner = Arc::new(ValidationApiInner {
            provider,
            consensus,
            payload_validator,
            executor_provider,
            disallow,
            validation_window,
            cached_state: Default::default(),
            task_spawner,
            metrics: Default::default(),
        });

        inner.metrics.disallow_size.set(inner.disallow.len() as f64);
        Self { inner }
    }

    /// Returns the cached reads for the given head hash.
    async fn cached_reads(&self, head: B256) -> CachedReads {
        let cache = self.inner.cached_state.read().await;
        if cache.0 == head { cache.1.clone() } else { Default::default() }
    }

    /// Updates the cached state for the given head hash.
    async fn update_cached_reads(&self, head: B256, cached_state: CachedReads) {
        let mut cache = self.inner.cached_state.write().await;
        if cache.0 == head {
            cache.1.extend(cached_state);
        } else {
            *cache = (head, cached_state)
        }
    }
}

impl<Provider, E> ValidationApi<Provider, E>
where
    Provider: BlockReaderIdExt<Header = <E::Primitives as NodePrimitives>::BlockHeader>
        + ChainSpecProvider<ChainSpec: EthereumHardforks>
        + StateProviderFactory
        + 'static,
    E: BlockExecutorProvider,
{
    /// Validates the given block and a [`BidTrace`] against it.
    pub async fn validate_message_against_block(
        &self,
        block: RecoveredBlock<<E::Primitives as NodePrimitives>::Block>,
        message: BidTrace,
        registered_gas_limit: u64,
        transaction_filter: TransactionFilter,
        is_mev_protect: bool,
    ) -> Result<(), ValidationApiError> {
        self.validate_message_against_header(block.sealed_header(), &message)?;

        self.consensus.validate_header_with_total_difficulty(block.sealed_header(), U256::MAX)?;
        self.consensus.validate_header(block.sealed_header())?;
        self.consensus.validate_block_pre_execution(block.sealed_block())?;

        if !self.disallow.is_empty() && transaction_filter == TransactionFilter::OFAC {
            if self.disallow.contains(&block.beneficiary()) {
                return Err(ValidationApiError::Blacklist(block.beneficiary()));
            }
            if self.disallow.contains(&message.proposer_fee_recipient) {
                return Err(ValidationApiError::Blacklist(message.proposer_fee_recipient));
            }
            for (sender, tx) in block.senders_iter().zip(block.body().transactions()) {
                if self.disallow.contains(sender) {
                    return Err(ValidationApiError::Blacklist(*sender));
                }
                if let Some(to) = tx.to() {
                    if self.disallow.contains(&to) {
                        return Err(ValidationApiError::Blacklist(to));
                    }
                }
            }
        }

        let latest_header =
            self.provider.latest_header()?.ok_or_else(|| ValidationApiError::MissingLatestBlock)?;

        let parent_header = if block.parent_hash() == latest_header.hash() {
            latest_header
        } else {
            // parent is not the latest header so we need to fetch it and ensure it's not too old
            let parent_header = self
                .provider
                .sealed_header_by_hash(block.parent_hash())?
                .ok_or_else(|| ValidationApiError::MissingParentBlock)?;

            if latest_header.number().saturating_sub(parent_header.number())
                > self.validation_window
            {
                return Err(ValidationApiError::BlockTooOld);
            }
            parent_header
        };

        self.consensus.validate_header_against_parent(block.sealed_header(), &parent_header)?;
        self.validate_gas_limit(registered_gas_limit, &parent_header, block.sealed_header())?;
        let parent_header_hash = parent_header.hash();
        let state_provider = self.provider.state_by_block_hash(parent_header_hash)?;

        let mut request_cache = self.cached_reads(parent_header_hash).await;

        let cached_db = request_cache.as_db_mut(StateProviderDatabase::new(&state_provider));
        let executor = self.executor_provider.executor(cached_db);

        let mut accessed_blacklisted = None;
        let output = executor.execute_with_state_closure(&block, |state| {
            if !self.disallow.is_empty() && transaction_filter == TransactionFilter::OFAC {
                // Check whether the submission interacted with any blacklisted account by scanning
                // the `State`'s cache that records everything read from database during execution.
                for account in state.cache.accounts.keys() {
                    if self.disallow.contains(account) {
                        accessed_blacklisted = Some(*account);
                    }
                }
            }
        })?;

        // update the cached reads
        self.update_cached_reads(parent_header_hash, request_cache).await;

        if let Some(account) = accessed_blacklisted {
            return Err(ValidationApiError::Blacklist(account));
        }

        self.consensus.validate_block_post_execution(&block, &output)?;

        self.ensure_payment(&block, &output, &message, is_mev_protect)?;

        let state_root =
            state_provider.state_root(state_provider.hashed_post_state(&output.state))?;

        if state_root != block.header().state_root() {
            return Err(ConsensusError::BodyStateRootDiff(
                GotExpected { got: state_root, expected: block.header().state_root() }.into(),
            )
            .into());
        }

        Ok(())
    }

    /// Ensures that fields of [`BidTrace`] match the fields of the [`SealedHeaderFor`].
    fn validate_message_against_header(
        &self,
        header: &SealedHeaderFor<E::Primitives>,
        message: &BidTrace,
    ) -> Result<(), ValidationApiError> {
        if header.hash() != message.block_hash {
            Err(ValidationApiError::BlockHashMismatch(GotExpected {
                got: message.block_hash,
                expected: header.hash(),
            }))
        } else if header.parent_hash() != message.parent_hash {
            Err(ValidationApiError::ParentHashMismatch(GotExpected {
                got: message.parent_hash,
                expected: header.parent_hash(),
            }))
        } else if header.gas_limit() != message.gas_limit {
            Err(ValidationApiError::GasLimitMismatch(GotExpected {
                got: message.gas_limit,
                expected: header.gas_limit(),
            }))
        } else if header.gas_used() != message.gas_used {
            return Err(ValidationApiError::GasUsedMismatch(GotExpected {
                got: message.gas_used,
                expected: header.gas_used(),
            }));
        } else {
            Ok(())
        }
    }

    /// Ensures that the chosen gas limit is the closest possible value for the validator's
    /// registered gas limit.
    ///
    /// Ref: <https://github.com/flashbots/builder/blob/a742641e24df68bc2fc476199b012b0abce40ffe/core/blockchain.go#L2474-L2477>
    fn validate_gas_limit(
        &self,
        registered_gas_limit: u64,
        parent_header: &SealedHeaderFor<E::Primitives>,
        header: &SealedHeaderFor<E::Primitives>,
    ) -> Result<(), ValidationApiError> {
        let max_gas_limit =
            parent_header.gas_limit() + parent_header.gas_limit() / GAS_LIMIT_BOUND_DIVISOR - 1;
        let min_gas_limit =
            parent_header.gas_limit() - parent_header.gas_limit() / GAS_LIMIT_BOUND_DIVISOR + 1;

        let best_gas_limit =
            std::cmp::max(min_gas_limit, std::cmp::min(max_gas_limit, registered_gas_limit));

        if best_gas_limit != header.gas_limit() {
            return Err(ValidationApiError::GasLimitMismatch(GotExpected {
                got: header.gas_limit(),
                expected: best_gas_limit,
            }));
        }

        Ok(())
    }

    /// Ensures that the proposer has received [`BidTrace::value`] for this block.
    ///
    /// Two validation paths are attempted, in this order, to keep behavior compatible with
    /// upstream:
    /// - Balance-delta path: accept if the proposer's balance increased by exactly the bid amount
    ///   (after withdrawals). This shortcut is disabled when MEV-protect is enabled.
    /// - Last-transaction path: otherwise (or always when MEV-protect is enabled), require that the
    ///   last tx is a direct payment to the proposer for exactly the bid amount with zero input and
    ///   zero tip.
    ///
    /// MEV-protect notes:
    /// - When MEV-protect is enabled, we require the last-transaction path to be satisfied and
    ///   enforce the 90% ratio check against coinbase delta.
    fn ensure_payment(
        &self,
        block: &SealedBlock<<E::Primitives as NodePrimitives>::Block>,
        output: &BlockExecutionOutput<<E::Primitives as NodePrimitives>::Receipt>,
        message: &BidTrace,
        is_mev_protect: bool,
    ) -> Result<(), ValidationApiError> {
        let (mut balance_before, balance_after) = if let Some(acc) =
            output.state.state.get(&message.proposer_fee_recipient)
        {
            let balance_before = acc.original_info.as_ref().map(|i| i.balance).unwrap_or_default();
            let balance_after = acc.info.as_ref().map(|i| i.balance).unwrap_or_default();

            (balance_before, balance_after)
        } else {
            // account might have balance but considering it zero is fine as long as we know
            // that balance have not changed
            (U256::ZERO, U256::ZERO)
        };

        if let Some(withdrawals) = block.body().withdrawals() {
            for withdrawal in withdrawals {
                if withdrawal.address == message.proposer_fee_recipient {
                    balance_before += withdrawal.amount_wei();
                }
            }
        }

        if balance_after >= balance_before.saturating_add(message.value) {
            // Balance-delta shortcut: accepted only when MEV-protect is NOT requested.
            // If MEV-protect is enabled, we still require the last-transaction path
            // (including all checks and the ratio requirement) to pass.
            if !is_mev_protect {
                return Ok(());
            }
        }

        let (receipt, tx) =
            output.receipts.last().zip(block.body().transactions().last()).ok_or_else(|| {
                if is_mev_protect {
                    ValidationApiError::MevProtectFailed(
                        "protectcheckfailed: mev-protect enabled but block invalid. proposer payment last tx missing. block accepted now but may be rejected in the future. contact t.me/ultrasoundrelay to learn more about supporting mev-protect.".to_string(),
                    )
                } else {
                    ValidationApiError::ProposerPayment
                }
            })?;

        if !receipt.status() {
            return Err(ValidationApiError::ProposerPayment);
        }

        if tx.to() != Some(message.proposer_fee_recipient) {
            return Err(ValidationApiError::ProposerPayment);
        }

        if tx.value() != message.value {
            return Err(ValidationApiError::ProposerPayment);
        }

        if !tx.input().is_empty() {
            return Err(ValidationApiError::ProposerPayment);
        }

        if let Some(block_base_fee) = block.header().base_fee_per_gas() {
            if tx.effective_tip_per_gas(block_base_fee).unwrap_or_default() != 0 {
                return Err(ValidationApiError::ProposerPayment);
            }
        }

        // MEV-protect additional check: ensure last tx value >= 90% of coinbase delta.
        // Note: the ratio is currently enforced only in this (last-transaction) path.
        if is_mev_protect {
            // Compute coinbase delta using state changes.
            let beneficiary = block.beneficiary();
            let (coinbase_before, coinbase_after) =
                if let Some(acc) = output.state.state.get(&beneficiary) {
                    let before = acc.original_info.as_ref().map(|i| i.balance).unwrap_or_default();
                    let after = acc.info.as_ref().map(|i| i.balance).unwrap_or_default();
                    (before, after)
                } else {
                    (U256::ZERO, U256::ZERO)
                };

            let coinbase_delta = if coinbase_after > coinbase_before {
                coinbase_after - coinbase_before
            } else {
                U256::ZERO
            };

            // Compute ratio against total: proposer_paid / (proposer_paid + coinbase_delta) >= 90%
            let proposer_paid = tx.value();
            let denom = proposer_paid.saturating_add(coinbase_delta);
            if !denom.is_zero() {
                let lhs = proposer_paid.saturating_mul(U256::from(10_000u64));
                let rhs = denom.saturating_mul(U256::from(9_000u64));
                if lhs < rhs {
                    let paid_bps = mev_ratio_bps(proposer_paid, coinbase_delta).unwrap_or(0);
                    let paid_pct = fmt_percent_2dp(paid_bps);
                    return Err(ValidationApiError::MevProtectFailed(format!(
                        "protectcheckfailed: mev-protect enabled but block invalid. ratio (proposer_paid / (proposer_paid + coinbase_delta)) = {paid_pct}, required >= 90.00%. block accepted now but may be rejected in the future. contact t.me/ultrasoundrelay to learn more about supporting mev-protect."
                    )));
                }
            }
        }

        Ok(())
    }

    /// Validates the given [`BlobsBundleV1`] and returns versioned hashes for blobs.
    pub fn validate_blobs_bundle(
        &self,
        mut blobs_bundle: BlobsBundleV1,
    ) -> Result<Vec<B256>, ValidationApiError> {
        if blobs_bundle.commitments.len() != blobs_bundle.proofs.len()
            || blobs_bundle.commitments.len() != blobs_bundle.blobs.len()
        {
            return Err(ValidationApiError::InvalidBlobsBundle);
        }

        let versioned_hashes = blobs_bundle
            .commitments
            .iter()
            .map(|c| kzg_to_versioned_hash(c.as_slice()))
            .collect::<Vec<_>>();

        let sidecar = blobs_bundle.pop_sidecar(blobs_bundle.blobs.len());

        sidecar.validate(&versioned_hashes, EnvKzgSettings::default().get())?;

        Ok(versioned_hashes)
    }

    /// Core logic for validating the builder submission v3
    async fn validate_builder_submission_v3(
        &self,
        request: BuilderBlockValidationRequestV3,
    ) -> Result<(), ValidationApiError> {
        let block = self.payload_validator.ensure_well_formed_payload(ExecutionData {
            payload: ExecutionPayload::V3(request.request.execution_payload),
            sidecar: ExecutionPayloadSidecar::v3(CancunPayloadFields {
                parent_beacon_block_root: request.parent_beacon_block_root,
                versioned_hashes: self.validate_blobs_bundle(request.request.blobs_bundle)?,
            }),
        })?;

        self.validate_message_against_block(
            block,
            request.request.message,
            request.registered_gas_limit,
            request.transaction_filter,
            request.is_mev_protect,
        )
        .await
    }

    /// Core logic for validating the builder submission v4
    async fn validate_builder_submission_v4(
        &self,
        request: BuilderBlockValidationRequestV4,
    ) -> Result<(), ValidationApiError> {
        let block = self.payload_validator.ensure_well_formed_payload(ExecutionData {
            payload: ExecutionPayload::V3(request.request.execution_payload),
            sidecar: ExecutionPayloadSidecar::v4(
                CancunPayloadFields {
                    parent_beacon_block_root: request.parent_beacon_block_root,
                    versioned_hashes: self.validate_blobs_bundle(request.request.blobs_bundle)?,
                },
                PraguePayloadFields {
                    requests: RequestsOrHash::Requests(
                        request.request.execution_requests.to_requests(),
                    ),
                },
            ),
        })?;

        self.validate_message_against_block(
            block,
            request.request.message,
            request.registered_gas_limit,
            request.transaction_filter,
            request.is_mev_protect,
        )
        .await
    }
}

#[async_trait]
impl<Provider, E> BlockSubmissionValidationApiServer for ValidationApi<Provider, E>
where
    Provider: BlockReaderIdExt<Header = <E::Primitives as NodePrimitives>::BlockHeader>
        + ChainSpecProvider<ChainSpec: EthereumHardforks>
        + StateProviderFactory
        + Clone
        + 'static,
    E: BlockExecutorProvider,
{
    async fn validate_builder_submission_v1(
        &self,
        _request: BuilderBlockValidationRequest,
    ) -> RpcResult<()> {
        warn!(target: "rpc::flashbots", "Method `flashbots_validateBuilderSubmissionV1` is not supported");
        Err(internal_rpc_err("unimplemented"))
    }

    async fn validate_builder_submission_v2(
        &self,
        _request: BuilderBlockValidationRequestV2,
    ) -> RpcResult<()> {
        warn!(target: "rpc::flashbots", "Method `flashbots_validateBuilderSubmissionV2` is not supported");
        Err(internal_rpc_err("unimplemented"))
    }

    /// Validates a block submitted to the relay
    async fn validate_builder_submission_v3(
        &self,
        request: BuilderBlockValidationRequestV3,
    ) -> RpcResult<()> {
        let this = self.clone();
        let (tx, rx) = oneshot::channel();

        self.task_spawner.spawn_blocking(Box::pin(async move {
            let result = Self::validate_builder_submission_v3(&this, request)
                .await
                .map_err(|err| internal_rpc_err(err.to_string()));
            let _ = tx.send(result);
        }));

        rx.await.map_err(|_| internal_rpc_err("Internal blocking task error"))?
    }

    /// Validates a block submitted to the relay
    async fn validate_builder_submission_v4(
        &self,
        request: BuilderBlockValidationRequestV4,
    ) -> RpcResult<()> {
        let this = self.clone();
        let (tx, rx) = oneshot::channel();

        self.task_spawner.spawn_blocking(Box::pin(async move {
            let result = Self::validate_builder_submission_v4(&this, request)
                .await
                .map_err(|err| internal_rpc_err(err.to_string()));
            let _ = tx.send(result);
        }));

        rx.await.map_err(|_| internal_rpc_err("Internal blocking task error"))?
    }
}

pub struct ValidationApiInner<Provider, E: BlockExecutorProvider> {
    /// The provider that can interact with the chain.
    provider: Provider,
    /// Consensus implementation.
    consensus: Arc<dyn FullConsensus<E::Primitives, Error = ConsensusError>>,
    /// Execution payload validator.
    payload_validator: Arc<
        dyn PayloadValidator<
                Block = <E::Primitives as NodePrimitives>::Block,
                ExecutionData = ExecutionData,
            >,
    >,
    /// Block executor factory.
    executor_provider: E,
    /// Set of disallowed addresses
    disallow: HashSet<Address>,
    /// The maximum block distance - parent to latest - allowed for validation
    validation_window: u64,
    /// Cached state reads to avoid redundant disk I/O across multiple validation attempts
    /// targeting the same state. Stores a tuple of (`block_hash`, `cached_reads`) for the
    /// latest head block state. Uses async `RwLock` to safely handle concurrent validation
    /// requests.
    cached_state: RwLock<(B256, CachedReads)>,
    /// Task spawner for blocking operations
    task_spawner: Box<dyn TaskSpawner>,
    /// Validation metrics
    metrics: ValidationMetrics,
}

impl<Provider, E: BlockExecutorProvider> fmt::Debug for ValidationApiInner<Provider, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidationApiInner").finish_non_exhaustive()
    }
}

/// Configuration for validation API.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationApiConfig {
    /// Disallowed addresses.
    pub disallow: HashSet<Address>,
    /// The maximum block distance - parent to latest - allowed for validation
    pub validation_window: u64,
}

impl ValidationApiConfig {
    /// Default validation blocks window of 3 blocks
    pub const DEFAULT_VALIDATION_WINDOW: u64 = 3;
}

impl Default for ValidationApiConfig {
    fn default() -> Self {
        Self { disallow: Default::default(), validation_window: Self::DEFAULT_VALIDATION_WINDOW }
    }
}

/// Errors thrown by the validation API.
#[derive(Debug, thiserror::Error)]
pub enum ValidationApiError {
    #[error("block gas limit mismatch: {_0}")]
    GasLimitMismatch(GotExpected<u64>),
    #[error("block gas used mismatch: {_0}")]
    GasUsedMismatch(GotExpected<u64>),
    #[error("block parent hash mismatch: {_0}")]
    ParentHashMismatch(GotExpected<B256>),
    #[error("block hash mismatch: {_0}")]
    BlockHashMismatch(GotExpected<B256>),
    #[error("missing latest block in database")]
    MissingLatestBlock,
    #[error("parent block not found")]
    MissingParentBlock,
    #[error("block is too old, outside validation window")]
    BlockTooOld,
    #[error("could not verify proposer payment")]
    ProposerPayment,
    #[error("invalid blobs bundle")]
    InvalidBlobsBundle,
    /// When the transaction signature is invalid
    #[error("invalid transaction signature")]
    InvalidTransactionSignature,
    #[error("block accesses blacklisted address: {_0}")]
    Blacklist(Address),
    #[error("{_0}")]
    MevProtectFailed(String),
    #[error(transparent)]
    Blob(#[from] BlobTransactionValidationError),
    #[error(transparent)]
    Consensus(#[from] ConsensusError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Execution(#[from] BlockExecutionError),
    #[error(transparent)]
    Payload(#[from] NewPayloadError),
}

impl From<ValidationApiError> for ErrorObject<'static> {
    fn from(error: ValidationApiError) -> Self {
        match error {
            ValidationApiError::GasLimitMismatch(_)
            | ValidationApiError::GasUsedMismatch(_)
            | ValidationApiError::ParentHashMismatch(_)
            | ValidationApiError::BlockHashMismatch(_)
            | ValidationApiError::Blacklist(_)
            | ValidationApiError::ProposerPayment
            | ValidationApiError::InvalidBlobsBundle
            | ValidationApiError::InvalidTransactionSignature
            | ValidationApiError::Blob(_)
            | ValidationApiError::MevProtectFailed(_) => invalid_params_rpc_err(error.to_string()),

            ValidationApiError::MissingLatestBlock
            | ValidationApiError::MissingParentBlock
            | ValidationApiError::BlockTooOld
            | ValidationApiError::Consensus(_)
            | ValidationApiError::Provider(_) => internal_rpc_err(error.to_string()),
            ValidationApiError::Execution(err) => match err {
                error @ BlockExecutionError::Validation(_) => {
                    invalid_params_rpc_err(error.to_string())
                }
                error @ BlockExecutionError::Internal(_) => internal_rpc_err(error.to_string()),
            },
            ValidationApiError::Payload(err) => match err {
                error @ NewPayloadError::Eth(_) => invalid_params_rpc_err(error.to_string()),
                error @ NewPayloadError::Other(_) => internal_rpc_err(error.to_string()),
            },
        }
    }
}

/// Metrics for the validation endpoint.
#[derive(Metrics)]
#[metrics(scope = "builder.validation")]
pub(crate) struct ValidationMetrics {
    /// The number of entries configured in the builder validation disallow list.
    pub(crate) disallow_size: Gauge,
}

#[cfg(test)]
mod tests {
    use super::{hash_disallow_list, mev_ratio_bps};
    use revm_primitives::{Address, U256};
    use std::collections::HashSet;

    #[test]
    fn test_hash_disallow_list_deterministic() {
        let mut addresses = HashSet::new();
        addresses.insert(Address::from([1u8; 20]));
        addresses.insert(Address::from([2u8; 20]));

        let hash1 = hash_disallow_list(&addresses);
        let hash2 = hash_disallow_list(&addresses);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_disallow_list_different_content() {
        let mut addresses1 = HashSet::new();
        addresses1.insert(Address::from([1u8; 20]));

        let mut addresses2 = HashSet::new();
        addresses2.insert(Address::from([2u8; 20]));

        let hash1 = hash_disallow_list(&addresses1);
        let hash2 = hash_disallow_list(&addresses2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hash_disallow_list_order_independent() {
        let mut addresses1 = HashSet::new();
        addresses1.insert(Address::from([1u8; 20]));
        addresses1.insert(Address::from([2u8; 20]));

        let mut addresses2 = HashSet::new();
        addresses2.insert(Address::from([2u8; 20])); // Different insertion order
        addresses2.insert(Address::from([1u8; 20]));

        let hash1 = hash_disallow_list(&addresses1);
        let hash2 = hash_disallow_list(&addresses2);

        assert_eq!(hash1, hash2);
    }

    #[test]
    //ensures parity with rbuilder hashing https://github.com/flashbots/rbuilder/blob/962c8444cdd490a216beda22c7eec164db9fc3ac/crates/rbuilder/src/live_builder/block_list_provider.rs#L248
    fn test_disallow_list_hash_rbuilder_parity() {
        let json = r#"["0x05E0b5B40B7b66098C2161A5EE11C5740A3A7C45","0x01e2919679362dFBC9ee1644Ba9C6da6D6245BB1","0x03893a7c7463AE47D46bc7f091665f1893656003","0x04DBA1194ee10112fE6C3207C0687DEf0e78baCf"]"#;
        let blocklist: Vec<Address> = serde_json::from_str(json).unwrap();
        let blocklist: HashSet<Address> = blocklist.into_iter().collect();
        let expected_hash = "ee14e9d115e182f61871a5a385ab2f32ecf434f3b17bdbacc71044810d89e608";
        let hash = hash_disallow_list(&blocklist);
        assert_eq!(expected_hash, hash);
    }

    #[test]
    fn test_mev_protect_missing_last_tx_prefix() {
        let err = super::ValidationApiError::MevProtectFailed(
            "protectcheckfailed: proposer_payment_last_tx_missing".to_string(),
        );
        assert!(err.to_string().starts_with("protectcheckfailed:"));
    }

    #[test]
    fn test_mev_protect_insufficient_ratio_prefix() {
        let err = super::ValidationApiError::MevProtectFailed(
            "protectcheckfailed: insufficient_proposer_payment paid_x100=8750 required_x100=9000"
                .to_string(),
        );
        assert!(err.to_string().starts_with("protectcheckfailed:"));
    }

    #[test]
    fn test_mev_ratio_bps() {
        // proposer_paid/(proposer_paid+coinbase_delta) with proposer_paid=9, coinbase_delta=1
        assert_eq!(mev_ratio_bps(U256::from(9u64), U256::from(1u64)), Some(9000));

        // 100/(100+11) => 100/111 ≈ 9009 bps
        assert_eq!(mev_ratio_bps(U256::from(100u64), U256::from(11u64)), Some(9009));

        // denom zero treated as None
        assert_eq!(mev_ratio_bps(U256::ZERO, U256::ZERO), None);
    }

    #[test]
    fn test_fmt_percent_2dp() {
        assert_eq!(super::fmt_percent_2dp(0), "0.00%");
        assert_eq!(super::fmt_percent_2dp(1), "0.01%");
        assert_eq!(super::fmt_percent_2dp(90_00), "90.00%");
        assert_eq!(super::fmt_percent_2dp(9_009), "90.09%");
    }
}
