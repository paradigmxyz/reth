use alloy_consensus::{
    BlobTransactionValidationError, BlockHeader, EnvKzgSettings, Transaction, TxReceipt,
};
use alloy_eips::{eip4844::kzg_to_versioned_hash, eip7685::RequestsOrHash};
use alloy_rpc_types_beacon::relay::{
    BidTrace, BuilderBlockValidationRequest, BuilderBlockValidationRequestV2,
    BuilderBlockValidationRequestV3, BuilderBlockValidationRequestV4,
};
use alloy_rpc_types_engine::{
    BlobsBundleV1, CancunPayloadFields, ExecutionPayload, ExecutionPayloadSidecar, PayloadError,
    PraguePayloadFields,
};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_consensus::{Consensus, FullConsensus, PostExecutionInput};
use reth_engine_primitives::PayloadValidator;
use reth_errors::{BlockExecutionError, ConsensusError, ProviderError};
use reth_evm::execute::{BlockExecutorProvider, Executor};
use reth_primitives::{GotExpected, NodePrimitives, SealedBlockWithSenders, SealedHeader};
use reth_primitives_traits::{constants::GAS_LIMIT_BOUND_DIVISOR, Block as _, BlockBody};
use reth_provider::{
    BlockExecutionInput, BlockExecutionOutput, BlockReaderIdExt, StateProviderFactory,
};
use reth_revm::{cached::CachedReads, database::StateProviderDatabase};
use reth_rpc_api::BlockSubmissionValidationApiServer;
use reth_rpc_server_types::result::internal_rpc_err;
use reth_tasks::TaskSpawner;
use revm_primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::{oneshot, RwLock};

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
        consensus: Arc<dyn FullConsensus<E::Primitives>>,
        executor_provider: E,
        config: ValidationApiConfig,
        task_spawner: Box<dyn TaskSpawner>,
        payload_validator: Arc<
            dyn PayloadValidator<Block = <E::Primitives as NodePrimitives>::Block>,
        >,
    ) -> Self {
        let ValidationApiConfig { disallow } = config;

        let inner = Arc::new(ValidationApiInner {
            provider,
            consensus,
            payload_validator,
            executor_provider,
            disallow,
            cached_state: Default::default(),
            task_spawner,
        });

        Self { inner }
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
        block: SealedBlockWithSenders<<E::Primitives as NodePrimitives>::Block>,
        message: BidTrace,
        registered_gas_limit: u64,
    ) -> Result<(), ValidationApiError> {
        self.validate_message_against_header(&block.header, &message)?;

        self.consensus.validate_header_with_total_difficulty(&block.header, U256::MAX)?;
        self.consensus.validate_header(&block.header)?;
        self.consensus.validate_block_pre_execution(&block)?;

        if !self.disallow.is_empty() {
            if self.disallow.contains(&block.beneficiary()) {
                return Err(ValidationApiError::Blacklist(block.beneficiary()))
            }
            if self.disallow.contains(&message.proposer_fee_recipient) {
                return Err(ValidationApiError::Blacklist(message.proposer_fee_recipient))
            }
            for (sender, tx) in block.senders.iter().zip(block.transactions()) {
                if self.disallow.contains(sender) {
                    return Err(ValidationApiError::Blacklist(*sender))
                }
                if let Some(to) = tx.to() {
                    if self.disallow.contains(&to) {
                        return Err(ValidationApiError::Blacklist(to))
                    }
                }
            }
        }

        let latest_header =
            self.provider.latest_header()?.ok_or_else(|| ValidationApiError::MissingLatestBlock)?;

        if latest_header.hash() != block.header.parent_hash() {
            return Err(ConsensusError::ParentHashMismatch(
                GotExpected { got: block.header.parent_hash(), expected: latest_header.hash() }
                    .into(),
            )
            .into())
        }
        self.consensus.validate_header_against_parent(&block.header, &latest_header)?;
        self.validate_gas_limit(registered_gas_limit, &latest_header, &block.header)?;

        let latest_header_hash = latest_header.hash();
        let state_provider = self.provider.state_by_block_hash(latest_header_hash)?;

        let mut request_cache = self.cached_reads(latest_header_hash).await;

        let cached_db = request_cache.as_db_mut(StateProviderDatabase::new(&state_provider));
        let executor = self.executor_provider.executor(cached_db);

        let block = block.unseal();
        let mut accessed_blacklisted = None;
        let output = executor.execute_with_state_closure(
            BlockExecutionInput::new(&block, U256::MAX),
            |state| {
                if !self.disallow.is_empty() {
                    for account in state.cache.accounts.keys() {
                        if self.disallow.contains(account) {
                            accessed_blacklisted = Some(*account);
                        }
                    }
                }
            },
        )?;

        // update the cached reads
        self.update_cached_reads(latest_header_hash, request_cache).await;

        if let Some(account) = accessed_blacklisted {
            return Err(ValidationApiError::Blacklist(account))
        }

        self.consensus.validate_block_post_execution(
            &block,
            PostExecutionInput::new(&output.receipts, &output.requests),
        )?;

        self.ensure_payment(&block, &output, &message)?;

        let state_root =
            state_provider.state_root(state_provider.hashed_post_state(&output.state))?;

        if state_root != block.header().state_root() {
            return Err(ConsensusError::BodyStateRootDiff(
                GotExpected { got: state_root, expected: block.header().state_root() }.into(),
            )
            .into())
        }

        Ok(())
    }

    /// Ensures that fields of [`BidTrace`] match the fields of the [`SealedHeader`].
    fn validate_message_against_header(
        &self,
        header: &SealedHeader<<E::Primitives as NodePrimitives>::BlockHeader>,
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
            }))
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
        parent_header: &SealedHeader<<E::Primitives as NodePrimitives>::BlockHeader>,
        header: &SealedHeader<<E::Primitives as NodePrimitives>::BlockHeader>,
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
            }))
        }

        Ok(())
    }

    /// Ensures that the proposer has received [`BidTrace::value`] for this block.
    ///
    /// Firstly attempts to verify the payment by checking the state changes, otherwise falls back
    /// to checking the latest block transaction.
    fn ensure_payment(
        &self,
        block: &<E::Primitives as NodePrimitives>::Block,
        output: &BlockExecutionOutput<<E::Primitives as NodePrimitives>::Receipt>,
        message: &BidTrace,
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

        if balance_after >= balance_before + message.value {
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

        if let Some(block_base_fee) = block.header().base_fee_per_gas() {
            if tx.effective_tip_per_gas(block_base_fee).unwrap_or_default() != 0 {
                return Err(ValidationApiError::ProposerPayment)
            }
        }

        Ok(())
    }

    /// Validates the given [`BlobsBundleV1`] and returns versioned hashes for blobs.
    pub fn validate_blobs_bundle(
        &self,
        mut blobs_bundle: BlobsBundleV1,
    ) -> Result<Vec<B256>, ValidationApiError> {
        if blobs_bundle.commitments.len() != blobs_bundle.proofs.len() ||
            blobs_bundle.commitments.len() != blobs_bundle.blobs.len()
        {
            return Err(ValidationApiError::InvalidBlobsBundle)
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
        let block = self
            .payload_validator
            .ensure_well_formed_payload(
                ExecutionPayload::V3(request.request.execution_payload),
                ExecutionPayloadSidecar::v3(CancunPayloadFields {
                    parent_beacon_block_root: request.parent_beacon_block_root,
                    versioned_hashes: self.validate_blobs_bundle(request.request.blobs_bundle)?,
                }),
            )?
            .try_seal_with_senders()
            .map_err(|_| ValidationApiError::InvalidTransactionSignature)?;

        self.validate_message_against_block(
            block,
            request.request.message,
            request.registered_gas_limit,
        )
        .await
    }

    /// Core logic for validating the builder submission v4
    async fn validate_builder_submission_v4(
        &self,
        request: BuilderBlockValidationRequestV4,
    ) -> Result<(), ValidationApiError> {
        let block = self
            .payload_validator
            .ensure_well_formed_payload(
                ExecutionPayload::V3(request.request.execution_payload),
                ExecutionPayloadSidecar::v4(
                    CancunPayloadFields {
                        parent_beacon_block_root: request.parent_beacon_block_root,
                        versioned_hashes: self
                            .validate_blobs_bundle(request.request.blobs_bundle)?,
                    },
                    PraguePayloadFields {
                        requests: RequestsOrHash::Requests(
                            request.request.execution_requests.to_requests(),
                        ),
                        target_blobs_per_block: request.request.target_blobs_per_block,
                    },
                ),
            )?
            .try_seal_with_senders()
            .map_err(|_| ValidationApiError::InvalidTransactionSignature)?;

        self.validate_message_against_block(
            block,
            request.request.message,
            request.registered_gas_limit,
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
        Err(internal_rpc_err("unimplemented"))
    }

    async fn validate_builder_submission_v2(
        &self,
        _request: BuilderBlockValidationRequestV2,
    ) -> RpcResult<()> {
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

#[derive(Debug)]
pub struct ValidationApiInner<Provider, E: BlockExecutorProvider> {
    /// The provider that can interact with the chain.
    provider: Provider,
    /// Consensus implementation.
    consensus: Arc<dyn FullConsensus<E::Primitives>>,
    /// Execution payload validator.
    payload_validator: Arc<dyn PayloadValidator<Block = <E::Primitives as NodePrimitives>::Block>>,
    /// Block executor factory.
    executor_provider: E,
    /// Set of disallowed addresses
    disallow: HashSet<Address>,
    /// Cached state reads to avoid redundant disk I/O across multiple validation attempts
    /// targeting the same state. Stores a tuple of (`block_hash`, `cached_reads`) for the
    /// latest head block state. Uses async `RwLock` to safely handle concurrent validation
    /// requests.
    cached_state: RwLock<(B256, CachedReads)>,
    /// Task spawner for blocking operations
    task_spawner: Box<dyn TaskSpawner>,
}

/// Configuration for validation API.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationApiConfig {
    /// Disallowed addresses.
    pub disallow: HashSet<Address>,
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
    #[error("could not verify proposer payment")]
    ProposerPayment,
    #[error("invalid blobs bundle")]
    InvalidBlobsBundle,
    /// When the transaction signature is invalid
    #[error("invalid transaction signature")]
    InvalidTransactionSignature,
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
    Payload(#[from] PayloadError),
}
