use self::{
    blob_cache::BlobValidationCache,
    parent_bal::{committed_bal, ParentBalDb, SharedBal},
};
use alloy_consensus::{
    BlobTransactionValidationError, BlockHeader, EnvKzgSettings, Transaction, TxReceipt,
};
use alloy_eip7928::{bal::DecodedBal, compute_block_access_list_hash};
use alloy_eips::eip7685::RequestsOrHash;
use alloy_primitives::{map::AddressSet, Address, B256, U256};
use alloy_rpc_types_beacon::relay::{
    BidTrace, BuilderBlockValidationRequest, BuilderBlockValidationRequestV2,
    BuilderBlockValidationRequestV3, BuilderBlockValidationRequestV4,
    BuilderBlockValidationRequestV5, BuilderBlockValidationRequestV6,
};
use alloy_rpc_types_engine::{
    BlobsBundleV1, BlobsBundleV2, CancunPayloadFields, ExecutionData, ExecutionPayload,
    ExecutionPayloadSidecar, PraguePayloadFields,
};
use async_trait::async_trait;
use core::fmt;
use jsonrpsee::core::RpcResult;
use jsonrpsee_types::error::ErrorObject;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_consensus::{Consensus, FullConsensus};
use reth_consensus_common::validation::MAX_RLP_BLOCK_SIZE;
use reth_engine_primitives::PayloadValidator;
use reth_errors::{BlockExecutionError, ConsensusError, ProviderError};
use reth_evm::{execute::Executor, ConfigureEvm, SenderRecoveryCache};
use reth_execution_types::BlockExecutionOutput;
use reth_metrics::{
    metrics,
    metrics::{gauge, Gauge},
    Metrics,
};
use reth_node_api::{NewPayloadError, PayloadTypes};
use reth_primitives_traits::{
    block::error::SealedBlockRecoveryError, BlockBody, GotExpected, NodePrimitives, RecoveredBlock,
    SealedBlock, SealedHeaderFor,
};
use reth_revm::{cached::CachedReads, database::StateProviderDatabase};
use reth_rpc_api::BlockSubmissionValidationApiServer;
use reth_rpc_eth_types::EthStateCache;
use reth_rpc_server_types::result::{internal_rpc_err, invalid_params_rpc_err};
use reth_storage_api::{
    BlockReaderIdExt, HashedPostStateProvider, StateProvider, StateProviderFactory,
};
use reth_tasks::Runtime;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, RwLock};
use tracing::{instrument::WithSubscriber, warn, Instrument};

mod blob_cache;
mod parent_bal;

/// The type that implements the `validation` rpc namespace trait
#[derive(Clone, derive_more::Deref)]
pub struct ValidationApi<Provider, E: ConfigureEvm, T: PayloadTypes> {
    #[deref]
    inner: Arc<ValidationApiInner<Provider, E, T>>,
    /// Shared decoded BAL cache for this API handle and its subsequent clones.
    parent_bal_cache: Option<EthStateCache<E::Primitives>>,
}

impl<Provider, E, T> ValidationApi<Provider, E, T>
where
    E: ConfigureEvm,
    T: PayloadTypes,
{
    /// Create a new instance of the [`ValidationApi`]
    ///
    /// If a `sender_recovery_cache` is given, the senders of submitted blocks are recovered
    /// through it, reusing senders that other node components have already recovered.
    pub fn new(
        provider: Provider,
        consensus: Arc<dyn FullConsensus<E::Primitives>>,
        evm_config: E,
        config: ValidationApiConfig,
        task_spawner: Runtime,
        payload_validator: Arc<
            dyn PayloadValidator<T, Block = <E::Primitives as NodePrimitives>::Block>,
        >,
        sender_recovery_cache: Option<SenderRecoveryCache>,
    ) -> Self {
        let ValidationApiConfig { disallow, validation_window } = config;

        let inner = Arc::new(ValidationApiInner {
            provider,
            consensus,
            payload_validator,
            evm_config,
            disallow,
            validation_window,
            cached_state: Default::default(),
            validated_blobs: Default::default(),
            task_spawner,
            sender_recovery_cache,
            metrics: Default::default(),
        });

        inner.metrics.disallow_size.set(inner.disallow.len() as f64);

        let disallow_hash = hash_disallow_list(&inner.disallow);
        let hash_gauge = gauge!("builder_validation_disallow_hash", "hash" => disallow_hash);
        hash_gauge.set(1.0);

        Self { inner, parent_bal_cache: None }
    }

    /// Enables upfront parent BAL reads through the shared ETH cache.
    ///
    /// Each eligible submission requests its parent's decoded BAL before entering the execution
    /// worker. Existing clones retain their configuration; omitting this uses provider-only reads.
    pub fn with_parent_bal_cache(mut self, cache: EthStateCache<E::Primitives>) -> Self {
        self.parent_bal_cache = Some(cache);
        self
    }

    /// Returns the cached reads for the given head hash.
    async fn cached_reads(&self, head: B256) -> CachedReads {
        let cache = self.inner.cached_state.read().await;
        if cache.0 == head {
            cache.1.clone()
        } else {
            Default::default()
        }
    }

    /// Updates the read cache without mixing values from different parents.
    async fn update_cached_reads(&self, head: B256, cached_state: CachedReads) {
        let mut cache = self.inner.cached_state.write().await;
        if cache.0 == head {
            cache.1.extend(cached_state);
        } else {
            *cache = (head, cached_state);
        }
    }

    /// Runs validation on the ordinary blocking pool after asynchronous BAL acquisition.
    ///
    /// Dropped requests are skipped before execution and stopped while awaiting cached reads.
    async fn spawn_validation<F>(&self, validation: F) -> Result<(), ValidationApiError>
    where
        F: Future<Output = Result<(), ValidationApiError>> + Send + 'static,
    {
        let (mut tx, rx) = oneshot::channel();
        self.task_spawner.spawn_blocking_task(
            async move {
                let result = tokio::select! {
                    biased;
                    _ = tx.closed() => return,
                    result = validation => result,
                };
                let _ = tx.send(result);
            }
            .in_current_span()
            .with_current_subscriber(),
        );
        rx.await.map_err(|_| {
            ProviderError::other(std::io::Error::other("Internal blocking task error"))
        })?
    }
}

impl<Provider, E, T> ValidationApi<Provider, E, T>
where
    Provider: BlockReaderIdExt<Header = <E::Primitives as NodePrimitives>::BlockHeader>
        + ChainSpecProvider<ChainSpec: EthereumHardforks>
        + StateProviderFactory
        + 'static,
    E: ConfigureEvm + 'static,
    T: PayloadTypes<ExecutionData = ExecutionData>,
{
    /// Validates the given block and a [`BidTrace`] against it.
    pub async fn validate_message_against_block(
        &self,
        block: RecoveredBlock<<E::Primitives as NodePrimitives>::Block>,
        message: BidTrace,
        registered_gas_limit: u64,
        decoded_bal: Option<DecodedBal>,
    ) -> Result<(), ValidationApiError> {
        let parent_bal = self.parent_bal(block.parent_hash(), block.timestamp()).await?;
        let this =
            Self { inner: self.inner.clone(), parent_bal_cache: self.parent_bal_cache.clone() };
        self.spawn_validation(async move {
            this.validate_message_against_block_inner(
                block,
                message,
                registered_gas_limit,
                decoded_bal,
                parent_bal,
            )
            .await
        })
        .await
    }

    /// Get the BAL before holding a blocking worker needed by the cache's provider fetches.
    async fn parent_bal(
        &self,
        hash: B256,
        child_timestamp: u64,
    ) -> Result<Option<SharedBal>, ValidationApiError> {
        let Some(cache) = &self.parent_bal_cache else { return Ok(None) };
        if !self.provider.chain_spec().is_amsterdam_active_at_timestamp(child_timestamp) {
            return Ok(None)
        }
        let start = Instant::now();
        let bal = tokio::select! {
            biased;
            _ = self.task_spawner.on_shutdown_signal().clone() => {
                return Err(ProviderError::other(std::io::Error::other("validation cancelled")).into())
            }
            result = tokio::time::timeout(PARENT_BAL_TIMEOUT, cache.get_bal(hash)) => match result {
                Ok(Ok(bal)) => bal,
                Ok(Err(err)) => {
                    warn!(target: "rpc::validation", %err, %hash, "Parent BAL unavailable; using state provider");
                    None
                }
                Err(err) => {
                    warn!(target: "rpc::validation", %err, %hash, "Parent BAL acquisition timed out; using state provider");
                    None
                }
            },
        };
        metrics::histogram!("builder.validation.parent_bal.load_seconds")
            .record(start.elapsed().as_secs_f64());
        metrics::counter!("builder.validation.parent_bal.loads").increment(1);
        Ok(bal)
    }

    /// Validation body for callers already executing on the appropriate blocking worker.
    async fn validate_message_against_block_inner(
        &self,
        block: RecoveredBlock<<E::Primitives as NodePrimitives>::Block>,
        message: BidTrace,
        registered_gas_limit: u64,
        decoded_bal: Option<DecodedBal>,
        parent_bal: Option<SharedBal>,
    ) -> Result<(), ValidationApiError> {
        self.validate_message_against_header(block.sealed_header(), &message)?;

        self.consensus.validate_header(block.sealed_header())?;
        self.consensus.validate_block_pre_execution(block.sealed_block())?;

        if !self.disallow.is_empty() {
            if self.disallow.contains(&block.beneficiary()) {
                return Err(ValidationApiError::Blacklist(block.beneficiary()))
            }
            if self.disallow.contains(&message.proposer_fee_recipient) {
                return Err(ValidationApiError::Blacklist(message.proposer_fee_recipient))
            }
            for (sender, tx) in block.senders_iter().zip(block.body().transactions()) {
                if self.disallow.contains(sender) {
                    return Err(ValidationApiError::Blacklist(*sender))
                }
                if let Some(to) = tx.to() &&
                    self.disallow.contains(&to)
                {
                    return Err(ValidationApiError::Blacklist(to))
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

            if latest_header.number().saturating_sub(parent_header.number()) >
                self.validation_window
            {
                return Err(ValidationApiError::BlockTooOld)
            }
            parent_header
        };

        self.consensus.validate_header_against_parent(block.sealed_header(), &parent_header)?;
        parent_header.validate_gas_limit(registered_gas_limit, block.gas_limit()).map_err(
            |err| {
                ValidationApiError::GasLimitMismatch(GotExpected {
                    got: err.got,
                    expected: err.expected,
                })
            },
        )?;

        // Ensure the submitted block access list does not exceed the block gas limit (EIP-7928)
        if let Some(decoded_bal) = decoded_bal {
            decoded_bal
                .as_bal()
                .validate_gas_limit(block.gas_limit())
                .map_err(ConsensusError::from)?;
        }

        let parent_header_hash = parent_header.hash();
        let state_provider = self.provider.state_by_block_hash(parent_header_hash)?;

        let mut request_cache = self.cached_reads(parent_header_hash).await;
        let parent_bal = committed_bal(parent_bal, parent_header.block_access_list_hash());
        let (output, block_access_list_hash) = {
            let parent_db = ParentBalDb::new(
                StateProviderDatabase::new((&state_provider).into_evm_state_provider()),
                parent_bal.as_ref().map(|bal| bal.as_bal().as_ref()),
            );
            let cached_db = request_cache.as_db_mut(parent_db);
            let mut executor = self.evm_config.batch_executor(cached_db);
            let result = executor.execute_one(&block)?;

            // The executor rebuilds the block access list whenever the block header contains a
            // BAL hash. Comparing the rebuilt hash against the header post execution also
            // commits to the submitted access list, because the header's BAL hash is derived
            // from the submitted bytes.
            let block_access_list_hash =
                executor.take_bal().map(|bal| compute_block_access_list_hash(&bal));

            let mut state = executor.into_state();
            if !self.disallow.is_empty() {
                // Check whether the submission interacted with any blacklisted account by
                // scanning the `State`'s cache that records everything read from database
                // during execution.
                for account in state.cache.accounts.keys() {
                    if self.disallow.contains(account) {
                        return Err(ValidationApiError::Blacklist(*account))
                    }
                }
            }

            let output = BlockExecutionOutput { state: state.take_bundle(), result };
            drop(state);
            (output, block_access_list_hash)
        };

        self.update_cached_reads(parent_header_hash, request_cache).await;

        self.consensus.validate_block_post_execution(
            &block,
            &output,
            None,
            block_access_list_hash,
        )?;

        self.ensure_payment(&block, &output, &message)?;

        let hashed_state = state_provider.hashed_post_state(&output.state)?;
        let state_root = state_provider.state_root(hashed_state)?;

        if state_root != block.header().state_root() {
            return Err(ConsensusError::BodyStateRootDiff(
                GotExpected { got: state_root, expected: block.header().state_root() }.into(),
            )
            .into())
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
            Err(ValidationApiError::GasUsedMismatch(GotExpected {
                got: message.gas_used,
                expected: header.gas_used(),
            }))
        } else {
            Ok(())
        }
    }

    /// Ensures that the proposer has received [`BidTrace::value`] for this block.
    ///
    /// Firstly attempts to verify the payment by checking the state changes, otherwise falls back
    /// to checking the latest block transaction.
    fn ensure_payment(
        &self,
        block: &SealedBlock<<E::Primitives as NodePrimitives>::Block>,
        output: &BlockExecutionOutput<<E::Primitives as NodePrimitives>::Receipt>,
        message: &BidTrace,
    ) -> Result<(), ValidationApiError> {
        // A zero-value bid promises the proposer nothing, so the payload owes nothing.
        if message.value.is_zero() {
            return Ok(())
        }

        let (mut balance_before, balance_after) = if let Some(acc) =
            output.state.account(&message.proposer_fee_recipient)
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
            return Ok(())
        }

        let (receipt, tx) = output
            .receipts
            .last()
            .zip(block.body().transactions().last())
            .ok_or(ValidationApiError::ProposerPayment)?;

        if !receipt.status() {
            return Err(ValidationApiError::ProposerPayment)
        }

        if tx.to() != Some(message.proposer_fee_recipient) {
            return Err(ValidationApiError::ProposerPayment)
        }

        if tx.value() != message.value {
            return Err(ValidationApiError::ProposerPayment)
        }

        if !tx.input().is_empty() {
            return Err(ValidationApiError::ProposerPayment)
        }

        if let Some(block_base_fee) = block.header().base_fee_per_gas() &&
            tx.effective_tip_per_gas(block_base_fee).unwrap_or_default() != 0
        {
            return Err(ValidationApiError::ProposerPayment)
        }

        Ok(())
    }

    /// Validates the given [`BlobsBundleV1`] and returns versioned hashes for blobs.
    pub fn validate_blobs_bundle(
        &self,
        blobs_bundle: BlobsBundleV1,
    ) -> Result<Vec<B256>, ValidationApiError> {
        let versioned_hashes = blobs_bundle.versioned_hashes();
        let sidecar =
            blobs_bundle.try_into_sidecar().map_err(|_| ValidationApiError::InvalidBlobsBundle)?;

        sidecar.validate(&versioned_hashes, EnvKzgSettings::default().get())?;
        Ok(versioned_hashes)
    }

    /// Validates the given [`BlobsBundleV2`] and returns versioned hashes for blobs.
    ///
    /// Exact blob, commitment, and cell-proof matches from recent submissions reuse KZG
    /// validation. The resulting hashes are still checked against the block's EIP-4844
    /// transactions during payload validation.
    pub fn validate_blobs_bundle_v2(
        &self,
        blobs_bundle: BlobsBundleV2,
    ) -> Result<Vec<B256>, ValidationApiError> {
        self.validated_blobs.validate(blobs_bundle)
    }

    /// Converts the payload into a block and recovers the transaction senders.
    ///
    /// Like the engine's `newPayload` handling, this leaves payload validation and conversion to
    /// the payload validator and recovers senders through the shared [`SenderRecoveryCache`] if
    /// one is configured, so senders already recovered on transaction ingress or payload
    /// execution are reused. Without a cache, this calls
    /// [`PayloadValidator::ensure_well_formed_payload`] so validator overrides are preserved.
    ///
    /// Senders recovered here are cached before the block is validated. Competing submissions for
    /// the same slot share most of their transactions, so the senders are likely to be needed again
    /// even if this submission is rejected, and only successfully recovered senders can be cached.
    fn recover_payload(
        &self,
        payload: ExecutionData,
    ) -> Result<RecoveredBlock<<E::Primitives as NodePrimitives>::Block>, ValidationApiError> {
        let Some(cache) = &self.sender_recovery_cache else {
            return self.payload_validator.ensure_well_formed_payload(payload).map_err(Into::into)
        };

        let block = self.payload_validator.convert_payload_to_block(payload)?;
        let recovered = match cache.recover_signers(block.body().transactions()) {
            Ok(senders) => Ok(RecoveredBlock::new_sealed(block, senders)),
            Err(_) => Err(SealedBlockRecoveryError::new(block)),
        };
        recovered.map_err(|err| NewPayloadError::Other(err.into()).into())
    }

    /// Core logic for validating the builder submission v3
    async fn validate_builder_submission_v3(
        &self,
        request: BuilderBlockValidationRequestV3,
        parent_bal: Option<SharedBal>,
    ) -> Result<(), ValidationApiError> {
        let block = self.recover_payload(ExecutionData {
            payload: ExecutionPayload::V3(request.request.execution_payload),
            sidecar: ExecutionPayloadSidecar::v3(CancunPayloadFields {
                parent_beacon_block_root: request.parent_beacon_block_root,
                versioned_hashes: self.validate_blobs_bundle(request.request.blobs_bundle)?,
            }),
        })?;

        self.validate_message_against_block_inner(
            block,
            request.request.message,
            request.registered_gas_limit,
            None,
            parent_bal,
        )
        .await
    }

    /// Core logic for validating the builder submission v4
    async fn validate_builder_submission_v4(
        &self,
        request: BuilderBlockValidationRequestV4,
        parent_bal: Option<SharedBal>,
    ) -> Result<(), ValidationApiError> {
        let block = self.recover_payload(ExecutionData {
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

        self.validate_message_against_block_inner(
            block,
            request.request.message,
            request.registered_gas_limit,
            None,
            parent_bal,
        )
        .await
    }

    /// Core logic for validating the builder submission v5
    async fn validate_builder_submission_v5(
        &self,
        request: BuilderBlockValidationRequestV5,
        parent_bal: Option<SharedBal>,
    ) -> Result<(), ValidationApiError> {
        let payload = ExecutionPayload::V3(request.request.execution_payload);
        validate_message_against_payload(&request.request.message, &payload)?;

        let block = self.recover_payload(ExecutionData {
            payload,
            sidecar: ExecutionPayloadSidecar::v4(
                CancunPayloadFields {
                    parent_beacon_block_root: request.parent_beacon_block_root,
                    versioned_hashes: self
                        .validate_blobs_bundle_v2(request.request.blobs_bundle)?,
                },
                PraguePayloadFields {
                    requests: RequestsOrHash::Requests(
                        request.request.execution_requests.to_requests(),
                    ),
                },
            ),
        })?;

        // Check block size as per EIP-7934 (only applies when Osaka hardfork is active)
        let chain_spec = self.provider.chain_spec();
        if chain_spec.is_osaka_active_at_timestamp(block.timestamp()) {
            let rlp_length = block.rlp_length();
            if rlp_length > MAX_RLP_BLOCK_SIZE {
                return Err(ValidationApiError::Consensus(ConsensusError::BlockTooLarge {
                    rlp_length,
                    max_rlp_length: MAX_RLP_BLOCK_SIZE,
                }));
            }
        }

        self.validate_message_against_block_inner(
            block,
            request.request.message,
            request.registered_gas_limit,
            None,
            parent_bal,
        )
        .await
    }

    /// Core logic for validating the builder submission v6
    async fn validate_builder_submission_v6(
        &self,
        request: BuilderBlockValidationRequestV6,
        parent_bal: Option<SharedBal>,
    ) -> Result<(), ValidationApiError> {
        let payload = ExecutionPayload::V4(request.request.execution_payload);
        validate_message_against_payload(&request.request.message, &payload)?;

        let decoded_bal =
            DecodedBal::from_rlp_bytes(payload.as_v4().unwrap().block_access_list.clone())
                .map_err(ValidationApiError::InvalidBlockAccessList)?;

        let block = self.recover_payload(ExecutionData {
            payload,
            sidecar: ExecutionPayloadSidecar::v4(
                CancunPayloadFields {
                    parent_beacon_block_root: request.parent_beacon_block_root,
                    versioned_hashes: self
                        .validate_blobs_bundle_v2(request.request.blobs_bundle)?,
                },
                PraguePayloadFields {
                    requests: RequestsOrHash::Requests(
                        request.request.execution_requests.to_requests(),
                    ),
                },
            ),
        })?;

        let chain_spec = self.provider.chain_spec();
        if chain_spec.is_osaka_active_at_timestamp(block.timestamp()) {
            let rlp_length = block.rlp_length();
            if rlp_length > MAX_RLP_BLOCK_SIZE {
                return Err(ValidationApiError::Consensus(ConsensusError::BlockTooLarge {
                    rlp_length,
                    max_rlp_length: MAX_RLP_BLOCK_SIZE,
                }));
            }
        }

        self.validate_message_against_block_inner(
            block,
            request.request.message,
            request.registered_gas_limit,
            Some(decoded_bal),
            parent_bal,
        )
        .await
    }
}

#[async_trait]
impl<Provider, E, T> BlockSubmissionValidationApiServer for ValidationApi<Provider, E, T>
where
    Provider: BlockReaderIdExt<Header = <E::Primitives as NodePrimitives>::BlockHeader>
        + ChainSpecProvider<ChainSpec: EthereumHardforks>
        + StateProviderFactory
        + Clone
        + 'static,
    E: ConfigureEvm + 'static,
    T: PayloadTypes<ExecutionData = ExecutionData>,
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
        let payload = &request.request.execution_payload.payload_inner.payload_inner;
        let parent_bal = self
            .parent_bal(payload.parent_hash, payload.timestamp)
            .await
            .map_err(ErrorObject::from)?;
        let this = self.clone();
        let validation =
            async move { Self::validate_builder_submission_v3(&this, request, parent_bal).await };
        self.spawn_validation(validation).await.map_err(ErrorObject::from)
    }

    /// Validates a block submitted to the relay
    async fn validate_builder_submission_v4(
        &self,
        request: BuilderBlockValidationRequestV4,
    ) -> RpcResult<()> {
        let payload = &request.request.execution_payload.payload_inner.payload_inner;
        let parent_bal = self
            .parent_bal(payload.parent_hash, payload.timestamp)
            .await
            .map_err(ErrorObject::from)?;
        let this = self.clone();
        let validation =
            async move { Self::validate_builder_submission_v4(&this, request, parent_bal).await };
        self.spawn_validation(validation).await.map_err(ErrorObject::from)
    }

    /// Validates a block submitted to the relay
    async fn validate_builder_submission_v5(
        &self,
        request: BuilderBlockValidationRequestV5,
    ) -> RpcResult<()> {
        let payload = &request.request.execution_payload.payload_inner.payload_inner;
        let parent_bal = self
            .parent_bal(payload.parent_hash, payload.timestamp)
            .await
            .map_err(ErrorObject::from)?;
        let this = self.clone();
        let validation =
            async move { Self::validate_builder_submission_v5(&this, request, parent_bal).await };
        self.spawn_validation(validation).await.map_err(ErrorObject::from)
    }

    /// Validates a block submitted to the relay
    async fn validate_builder_submission_v6(
        &self,
        request: BuilderBlockValidationRequestV6,
    ) -> RpcResult<()> {
        let payload = &request.request.execution_payload.payload_inner.payload_inner.payload_inner;
        let parent_bal = self
            .parent_bal(payload.parent_hash, payload.timestamp)
            .await
            .map_err(ErrorObject::from)?;
        let this = self.clone();
        let validation =
            async move { Self::validate_builder_submission_v6(&this, request, parent_bal).await };
        self.spawn_validation(validation).await.map_err(ErrorObject::from)
    }
}

pub struct ValidationApiInner<Provider, E: ConfigureEvm, T: PayloadTypes> {
    /// The provider that can interact with the chain.
    provider: Provider,
    /// Consensus implementation.
    consensus: Arc<dyn FullConsensus<E::Primitives>>,
    /// Execution payload validator.
    payload_validator:
        Arc<dyn PayloadValidator<T, Block = <E::Primitives as NodePrimitives>::Block>>,
    /// Block executor factory.
    evm_config: E,
    /// Set of disallowed addresses
    disallow: AddressSet,
    /// The maximum block distance - parent to latest - allowed for validation
    validation_window: u64,
    /// Cached state reads targeting the same parent hash.
    cached_state: RwLock<(B256, CachedReads)>,
    /// Recently validated blob, commitment, and cell-proof tuples shared by V2 submissions.
    validated_blobs: BlobValidationCache,
    /// Task spawner for blocking operations
    task_spawner: Runtime,
    /// Cache of recovered transaction senders shared with transaction ingress and payload
    /// execution, if enabled.
    sender_recovery_cache: Option<SenderRecoveryCache>,
    /// Validation metrics
    metrics: ValidationMetrics,
}

/// Ensures that the raw execution payload fields match the corresponding [`BidTrace`] fields.
fn validate_message_against_payload(
    message: &BidTrace,
    payload: &ExecutionPayload,
) -> Result<(), ValidationApiError> {
    let payload = payload.as_v1();

    if payload.block_hash != message.block_hash {
        Err(ValidationApiError::BlockHashMismatch(GotExpected {
            got: message.block_hash,
            expected: payload.block_hash,
        }))
    } else if payload.parent_hash != message.parent_hash {
        Err(ValidationApiError::ParentHashMismatch(GotExpected {
            got: message.parent_hash,
            expected: payload.parent_hash,
        }))
    } else if payload.gas_limit != message.gas_limit {
        Err(ValidationApiError::GasLimitMismatch(GotExpected {
            got: message.gas_limit,
            expected: payload.gas_limit,
        }))
    } else if payload.gas_used != message.gas_used {
        Err(ValidationApiError::GasUsedMismatch(GotExpected {
            got: message.gas_used,
            expected: payload.gas_used,
        }))
    } else {
        Ok(())
    }
}

/// Calculates a deterministic hash of the blocklist for change detection.
///
/// This function sorts addresses to ensure deterministic output regardless of
/// insertion order, then computes a SHA256 hash of the concatenated addresses.
fn hash_disallow_list(disallow: &AddressSet) -> String {
    let mut sorted: Vec<_> = disallow.iter().collect();
    sorted.sort_unstable(); // sort for deterministic hashing

    let mut hasher = Sha256::new();
    for addr in sorted {
        hasher.update(addr.as_slice());
    }

    format!("{:x}", hasher.finalize())
}

impl<Provider, E: ConfigureEvm, T: PayloadTypes> fmt::Debug for ValidationApiInner<Provider, E, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidationApiInner").finish_non_exhaustive()
    }
}

impl<Provider, E: ConfigureEvm, T: PayloadTypes> fmt::Debug for ValidationApi<Provider, E, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidationApi")
            .field("inner", &self.inner)
            .field("parent_bal_enabled", &self.parent_bal_cache.is_some())
            .finish()
    }
}

/// Optional BAL lookup must not delay provider-only fallback indefinitely.
const PARENT_BAL_TIMEOUT: Duration = Duration::from_secs(1);

/// Configuration for validation API.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationApiConfig {
    /// Disallowed addresses.
    pub disallow: AddressSet,
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
    #[error("invalid block access list: {_0}")]
    InvalidBlockAccessList(alloy_rlp::Error),
    #[error("block accesses blacklisted address: {_0}")]
    Blacklist(Address),
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
            ValidationApiError::GasLimitMismatch(_) |
            ValidationApiError::GasUsedMismatch(_) |
            ValidationApiError::ParentHashMismatch(_) |
            ValidationApiError::BlockHashMismatch(_) |
            ValidationApiError::Blacklist(_) |
            ValidationApiError::ProposerPayment |
            ValidationApiError::InvalidBlobsBundle |
            ValidationApiError::InvalidBlockAccessList(_) |
            ValidationApiError::Blob(_) => invalid_params_rpc_err(error.to_string()),

            ValidationApiError::Consensus(
                error @ (ConsensusError::BlockAccessListCostMoreThanGasLimit(_) |
                ConsensusError::BlockAccessListHashMismatch(_)),
            ) => invalid_params_rpc_err(error.to_string()),
            ValidationApiError::MissingLatestBlock |
            ValidationApiError::MissingParentBlock |
            ValidationApiError::BlockTooOld |
            ValidationApiError::Consensus(_) |
            ValidationApiError::Provider(_) => internal_rpc_err(error.to_string()),
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
    use super::*;
    use alloy_consensus::{BlockHeader, Header};
    use alloy_eips::NumHash;
    use alloy_primitives::{Address, Bytes, B256, U256};
    use alloy_rpc_types_beacon::relay::BidTrace;
    use alloy_rpc_types_engine::{ExecutionData, ExecutionPayload, ExecutionPayloadV1};
    use reth_consensus::noop::NoopConsensus;
    use reth_engine_primitives::PayloadValidator;
    use reth_ethereum_engine_primitives::EthPayloadTypes;
    use reth_ethereum_primitives::{Block, EthPrimitives};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_execution_types::BlockExecutionOutput;
    use reth_node_api::NewPayloadError;
    use reth_primitives_traits::{RecoveredBlock, SealedBlock, SealedHeader};
    use reth_provider::{test_utils::MockEthProvider, InMemoryBalStore};
    use reth_revm::db::{states::bundle_state::BundleState, AccountStatus, BundleAccount};
    use reth_rpc_eth_types::EthStateCacheConfig;
    use reth_storage_api::{BalStoreHandle, RawBal};
    use reth_tasks::Runtime;
    use revm::{
        database::EmptyDB,
        state::{
            bal::{AccountBal, Bal, BalWrites, BlockAccessIndex},
            AccountInfo,
        },
        DatabaseRef,
    };
    use std::{
        sync::{mpsc, Arc},
        time::Duration,
    };

    fn test_execution_payload() -> ExecutionPayload {
        ExecutionPayload::V1(ExecutionPayloadV1 {
            parent_hash: B256::repeat_byte(0x11),
            fee_recipient: Address::ZERO,
            state_root: B256::ZERO,
            receipts_root: B256::ZERO,
            logs_bloom: Default::default(),
            prev_randao: B256::ZERO,
            block_number: 1,
            gas_limit: 30_000_000,
            gas_used: 15_000_000,
            timestamp: 1,
            extra_data: Default::default(),
            base_fee_per_gas: Default::default(),
            block_hash: B256::repeat_byte(0x22),
            transactions: Default::default(),
        })
    }

    fn matching_bid_trace(payload: &ExecutionPayload) -> BidTrace {
        let payload = payload.as_v1();
        BidTrace {
            parent_hash: payload.parent_hash,
            block_hash: payload.block_hash,
            gas_limit: payload.gas_limit,
            gas_used: payload.gas_used,
            ..Default::default()
        }
    }

    #[test]
    fn test_validate_message_against_payload_block_hash_mismatch() {
        let payload = test_execution_payload();
        let mut message = matching_bid_trace(&payload);
        message.block_hash = B256::repeat_byte(0x33);

        let err = validate_message_against_payload(&message, &payload).unwrap_err();
        let ValidationApiError::BlockHashMismatch(mismatch) = err else {
            panic!("unexpected error: {err}")
        };
        assert_eq!(mismatch.got, message.block_hash);
        assert_eq!(mismatch.expected, payload.block_hash());
    }

    #[test]
    fn test_validate_message_against_payload_parent_hash_mismatch() {
        let payload = test_execution_payload();
        let mut message = matching_bid_trace(&payload);
        message.parent_hash = B256::repeat_byte(0x33);

        let err = validate_message_against_payload(&message, &payload).unwrap_err();
        let ValidationApiError::ParentHashMismatch(mismatch) = err else {
            panic!("unexpected error: {err}")
        };
        assert_eq!(mismatch.got, message.parent_hash);
        assert_eq!(mismatch.expected, payload.parent_hash());
    }

    #[test]
    fn test_validate_message_against_payload_gas_limit_mismatch() {
        let payload = test_execution_payload();
        let mut message = matching_bid_trace(&payload);
        message.gas_limit += 1;

        let err = validate_message_against_payload(&message, &payload).unwrap_err();
        let ValidationApiError::GasLimitMismatch(mismatch) = err else {
            panic!("unexpected error: {err}")
        };
        assert_eq!(mismatch.got, message.gas_limit);
        assert_eq!(mismatch.expected, payload.gas_limit());
    }

    #[test]
    fn test_validate_message_against_payload_gas_used_mismatch() {
        let payload = test_execution_payload();
        let mut message = matching_bid_trace(&payload);
        message.gas_used += 1;

        let err = validate_message_against_payload(&message, &payload).unwrap_err();
        let ValidationApiError::GasUsedMismatch(mismatch) = err else {
            panic!("unexpected error: {err}")
        };
        assert_eq!(mismatch.got, message.gas_used);
        assert_eq!(mismatch.expected, payload.as_v1().gas_used);
    }

    #[test]
    fn test_hash_disallow_list_deterministic() {
        let mut addresses = AddressSet::default();
        addresses.insert(Address::repeat_byte(1u8));
        addresses.insert(Address::repeat_byte(2u8));

        let hash1 = hash_disallow_list(&addresses);
        let hash2 = hash_disallow_list(&addresses);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_disallow_list_different_content() {
        let mut addresses1 = AddressSet::default();
        addresses1.insert(Address::repeat_byte(1u8));

        let mut addresses2 = AddressSet::default();
        addresses2.insert(Address::repeat_byte(2u8));

        let hash1 = hash_disallow_list(&addresses1);
        let hash2 = hash_disallow_list(&addresses2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hash_disallow_list_order_independent() {
        let mut addresses1 = AddressSet::default();
        addresses1.insert(Address::repeat_byte(1u8));
        addresses1.insert(Address::repeat_byte(2u8));

        let mut addresses2 = AddressSet::default();
        addresses2.insert(Address::repeat_byte(2u8)); // Different insertion order
        addresses2.insert(Address::repeat_byte(1u8));

        let hash1 = hash_disallow_list(&addresses1);
        let hash2 = hash_disallow_list(&addresses2);

        assert_eq!(hash1, hash2);
    }

    #[test]
    //ensures parity with rbuilder hashing https://github.com/flashbots/rbuilder/blob/962c8444cdd490a216beda22c7eec164db9fc3ac/crates/rbuilder/src/live_builder/block_list_provider.rs#L248
    fn test_disallow_list_hash_rbuilder_parity() {
        let json = r#"["0x05E0b5B40B7b66098C2161A5EE11C5740A3A7C45","0x01e2919679362dFBC9ee1644Ba9C6da6D6245BB1","0x03893a7c7463AE47D46bc7f091665f1893656003","0x04DBA1194ee10112fE6C3207C0687DEf0e78baCf"]"#;
        let blocklist: Vec<Address> = serde_json::from_str(json).unwrap();
        let blocklist: AddressSet = blocklist.into_iter().collect();
        let expected_hash = "ee14e9d115e182f61871a5a385ab2f32ecf434f3b17bdbacc71044810d89e608";
        let hash = hash_disallow_list(&blocklist);
        assert_eq!(expected_hash, hash);
    }

    /// Only [`ValidationApi::validate_message_against_block`] is exercised below, which never
    /// converts a payload.
    #[derive(Debug)]
    struct UnusedPayloadValidator;

    impl PayloadValidator<EthPayloadTypes> for UnusedPayloadValidator {
        type Block = Block;

        fn convert_payload_to_block(
            &self,
            _payload: ExecutionData,
        ) -> Result<SealedBlock<Self::Block>, NewPayloadError> {
            unimplemented!()
        }
    }

    #[derive(Debug)]
    struct SpecializedPayloadValidator;

    impl PayloadValidator<EthPayloadTypes> for SpecializedPayloadValidator {
        type Block = Block;

        fn convert_payload_to_block(
            &self,
            _payload: ExecutionData,
        ) -> Result<SealedBlock<Self::Block>, NewPayloadError> {
            panic!("the specialized recovery method must be used without a cache")
        }

        fn ensure_well_formed_payload(
            &self,
            _payload: ExecutionData,
        ) -> Result<RecoveredBlock<Self::Block>, NewPayloadError> {
            Ok(SealedBlock::seal_slow(Block::default()).try_recover().unwrap())
        }
    }

    #[test]
    fn recover_payload_without_cache_uses_validator_override() {
        let api = ValidationApi::new(
            MockEthProvider::default(),
            NoopConsensus::arc(),
            EthEvmConfig::mainnet(),
            ValidationApiConfig::default(),
            Runtime::test(),
            Arc::new(SpecializedPayloadValidator),
            None,
        );

        api.recover_payload(ExecutionData {
            payload: test_execution_payload(),
            sidecar: Default::default(),
        })
        .unwrap();
    }

    fn test_validation_api(
        provider: MockEthProvider,
    ) -> ValidationApi<MockEthProvider, EthEvmConfig, EthPayloadTypes> {
        ValidationApi::new(
            provider,
            NoopConsensus::arc(),
            EthEvmConfig::mainnet(),
            ValidationApiConfig::default(),
            Runtime::test(),
            Arc::new(UnusedPayloadValidator),
            None,
        )
    }

    /// A submission whose payload pays the proposer nothing, like a trustless ePBS bid: there the
    /// payment settles from the builder's stake on the consensus layer.
    fn payment_free_submission() -> (MockEthProvider, RecoveredBlock<Block>, BidTrace) {
        let provider = MockEthProvider::default();

        let parent = Header { gas_limit: 30_000_000, ..Default::default() };
        let parent = SealedHeader::seal_slow(parent);
        provider.add_block(
            parent.hash(),
            Block { header: parent.clone_header(), body: Default::default() },
        );

        let header = Header {
            parent_hash: parent.hash(),
            number: parent.number() + 1,
            gas_limit: parent.gas_limit(),
            timestamp: parent.timestamp() + 12,
            ..Default::default()
        };
        let block = SealedBlock::seal_slow(Block { header, body: Default::default() })
            .try_recover()
            .unwrap();
        provider.state_roots.lock().push(block.state_root());

        let message = BidTrace {
            parent_hash: block.parent_hash(),
            block_hash: block.hash(),
            gas_limit: block.gas_limit(),
            gas_used: block.gas_used(),
            proposer_fee_recipient: Address::repeat_byte(0x42),
            value: U256::from(1_000_000_000_000_000_000u64),
            ..Default::default()
        };

        (provider, block, message)
    }

    #[tokio::test]
    async fn test_payment_check_runs_for_a_nonzero_bid() {
        let (provider, block, message) = payment_free_submission();
        let registered_gas_limit = block.gas_limit();

        let err = test_validation_api(provider)
            .validate_message_against_block(block, message, registered_gas_limit, None)
            .await
            .unwrap_err();

        assert!(matches!(err, ValidationApiError::ProposerPayment), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn test_zero_value_bid_skips_the_payment_check() {
        let (provider, block, mut message) = payment_free_submission();
        let registered_gas_limit = block.gas_limit();
        message.value = U256::ZERO;

        test_validation_api(provider)
            .validate_message_against_block(block, message, registered_gas_limit, None)
            .await
            .unwrap();
    }

    /// A zero-value bid promises the proposer nothing, so there is nothing to verify. The
    /// balance-delta branch already lets one through whenever the fee recipient's balance does
    /// not fall -- but a fee recipient that merely sends a transaction of its own in the same
    /// block ends it poorer, drops through to the last-transaction fallback and is rejected.
    #[test]
    fn test_zero_value_bid_is_accepted_when_the_fee_recipient_spends() {
        let (provider, block, mut message) = payment_free_submission();
        message.value = U256::ZERO;

        let output = BlockExecutionOutput {
            result: Default::default(),
            state: fee_recipient_spent(message.proposer_fee_recipient),
        };

        test_validation_api(provider)
            .ensure_payment(block.sealed_block(), &output, &message)
            .unwrap();
    }

    /// Execution state for a block in which `address` ended up poorer than it started.
    fn fee_recipient_spent(address: Address) -> BundleState {
        let balance =
            |wei: u64| Some(AccountInfo { balance: U256::from(wei), ..Default::default() });

        let mut state = BundleState::default();
        state.state.insert(
            address,
            BundleAccount {
                original_info: balance(1_000),
                info: balance(999),
                storage: Default::default(),
                status: AccountStatus::Changed,
            },
        );
        state
    }

    #[tokio::test]
    async fn cached_reads_are_isolated_by_parent_without_premature_eviction() {
        let api = test_validation_api(MockEthProvider::default());
        let first = B256::repeat_byte(1);
        let fork = B256::repeat_byte(2);
        let mut reads = api.cached_reads(first).await;
        reads.insert_account(
            Address::ZERO,
            AccountInfo { nonce: 9, ..Default::default() },
            Default::default(),
        );
        api.update_cached_reads(first, reads).await;
        assert!(api.cached_reads(fork).await.accounts.is_empty());
        assert_eq!(
            api.cached_reads(first).await.accounts[&Address::ZERO].info.as_ref().unwrap().nonce,
            9
        );
        api.update_cached_reads(fork, Default::default()).await;
        assert!(api.cached_reads(first).await.accounts.is_empty());
    }

    fn storage_bal(value: u64) -> SharedBal {
        let mut account = AccountBal::default();
        account.storage.storage.insert(
            U256::from(1),
            BalWrites::new(vec![(BlockAccessIndex::new(4), U256::from(value))]),
        );
        let mut bal = Bal::default();
        bal.accounts.insert(Address::repeat_byte(1), account);
        let raw: Bytes = alloy_rlp::encode(bal.clone().into_alloy_bal()).into();
        let hash = alloy_primitives::keccak256(&raw);
        Arc::new(DecodedBal::new_unchecked(Arc::new(bal), raw, hash))
    }

    fn parent_bal_api(
        mut provider: MockEthProvider,
    ) -> ValidationApi<MockEthProvider, EthEvmConfig, EthPayloadTypes> {
        let chain = Arc::new(
            reth_chainspec::ChainSpecBuilder::from(&*reth_chainspec::MAINNET)
                .amsterdam_activated()
                .build(),
        );
        provider.chain_spec = chain.clone();
        let cache = EthStateCache::<EthPrimitives>::spawn_with(
            provider.clone(),
            EthStateCacheConfig::default(),
            Runtime::test(),
        );
        let mut api = test_validation_api(provider).with_parent_bal_cache(cache);
        Arc::get_mut(&mut api.inner).unwrap().evm_config = EthEvmConfig::new(chain);
        api
    }

    #[tokio::test]
    async fn upfront_requests_reuse_the_eth_cache_and_select_the_exact_parent() {
        let api = parent_bal_api(MockEthProvider::default());
        let cache = api.parent_bal_cache.as_ref().unwrap().clone();
        let first = B256::repeat_byte(1);
        let fork = B256::repeat_byte(2);
        for (hash, value) in [(first, 42), (fork, 77)] {
            cache.insert_bal(hash, Arc::unwrap_or_clone(storage_bal(value)));
        }
        let original = api.parent_bal(first, 1).await.unwrap().unwrap();
        // Even with a populated read cache, the next submission asks EthStateCache again.
        api.update_cached_reads(first, Default::default()).await;
        let repeated = api.parent_bal(first, 1).await.unwrap().unwrap();
        assert!(Arc::ptr_eq(&original, &repeated));
        let fork_bal = api.parent_bal(fork, 1).await.unwrap().unwrap();
        assert_eq!(
            ParentBalDb::new(EmptyDB::default(), Some(fork_bal.as_bal().as_ref()))
                .storage_ref(Address::repeat_byte(1), U256::from(1))
                .unwrap(),
            U256::from(77)
        );
        assert!(api.parent_bal(B256::repeat_byte(3), 1).await.unwrap().is_none());
        let unconfigured = test_validation_api(MockEthProvider::default());
        let original_handle = unconfigured.clone();
        let configured = unconfigured.with_parent_bal_cache(cache);
        assert!(original_handle.parent_bal_cache.is_none());
        assert!(configured.parent_bal_cache.is_some());
    }

    #[tokio::test]
    async fn unavailable_and_pre_amsterdam_bal_use_provider_fallback() {
        let provider = MockEthProvider::default();
        let cache_runtime = Runtime::test();
        let cache = EthStateCache::<EthPrimitives>::spawn_with(
            provider.clone(),
            EthStateCacheConfig::default(),
            cache_runtime.clone(),
        );
        cache_runtime.graceful_shutdown();
        cache_runtime.on_shutdown_signal().clone().await;
        let pre_amsterdam =
            test_validation_api(provider.clone()).with_parent_bal_cache(cache.clone());
        assert!(pre_amsterdam.parent_bal(B256::ZERO, 1).await.unwrap().is_none());
        let active = parent_bal_api(provider).with_parent_bal_cache(cache);
        assert!(active.parent_bal(B256::ZERO, 1).await.unwrap().is_none());
    }

    fn single_blocking_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn upfront_public_validation_does_not_hold_the_bal_fetch_worker() {
        let runtime = single_blocking_runtime();
        runtime.block_on(async {
            let mut provider = MockEthProvider::default();
            let bal = storage_bal(42);
            let parent = SealedHeader::seal_slow(Header {
                gas_limit: 30_000_000,
                block_access_list_hash: Some(bal.hash()),
                ..Default::default()
            });
            provider.add_block(
                parent.hash(),
                Block { header: parent.clone_header(), body: Default::default() },
            );
            provider.bal_store = BalStoreHandle::new(InMemoryBalStore::new(Default::default()));
            provider
                .bal_store
                .insert(NumHash::new(0, parent.hash()), RawBal::from(bal.as_raw().clone()))
                .unwrap();
            let block = RecoveredBlock::new_unhashed(
                Block {
                    header: Header {
                        parent_hash: parent.hash(),
                        number: 1,
                        timestamp: 12,
                        gas_limit: parent.gas_limit(),
                        base_fee_per_gas: Some(0),
                        excess_blob_gas: Some(0),
                        parent_beacon_block_root: Some(B256::ZERO),
                        block_access_list_hash: Some(B256::ZERO),
                        ..Default::default()
                    },
                    body: reth_ethereum_primitives::BlockBody {
                        withdrawals: Some(Default::default()),
                        ..Default::default()
                    },
                },
                vec![],
            );
            let message = BidTrace {
                block_hash: block.hash(),
                parent_hash: parent.hash(),
                gas_limit: block.gas_limit(),
                ..Default::default()
            };
            let api = parent_bal_api(provider.clone());
            for _ in 0..2 {
                provider.state_roots.lock().push(block.state_root());
                tokio::time::timeout(
                    Duration::from_secs(2),
                    api.validate_message_against_block(
                        block.clone(),
                        message.clone(),
                        block.gas_limit(),
                        None,
                    ),
                )
                .await
                .unwrap()
                .unwrap();
            }
            let cached = api.parent_bal(parent.hash(), 12).await.unwrap().unwrap();
            assert_eq!(cached.hash(), bal.hash());
        });
        runtime.shutdown_timeout(Duration::from_secs(2));
    }

    #[test]
    fn timed_out_upfront_fetch_falls_back_and_later_requests_retry() {
        let runtime = single_blocking_runtime();
        runtime.block_on(async {
            let api = parent_bal_api(MockEthProvider::default());
            let (entered_tx, entered_rx) = oneshot::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let blocker = api.task_spawner.spawn_blocking(move || {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            });
            tokio::time::timeout(Duration::from_secs(2), entered_rx).await.unwrap().unwrap();
            let hash = B256::repeat_byte(1);
            assert!(tokio::time::timeout(Duration::from_secs(2), api.parent_bal(hash, 1))
                .await
                .unwrap()
                .unwrap()
                .is_none());
            release_tx.send(()).unwrap();
            blocker.await.unwrap();
            api.parent_bal_cache
                .as_ref()
                .unwrap()
                .insert_bal(hash, Arc::unwrap_or_clone(storage_bal(42)));
            assert_eq!(
                api.parent_bal(hash, 1).await.unwrap().unwrap().hash(),
                storage_bal(42).hash()
            );
        });
        runtime.shutdown_timeout(Duration::from_secs(2));
    }
}
