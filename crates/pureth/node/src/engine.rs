use crate::ExecutionBuiltPayload;
use alloy_consensus::{BlockBody as AlloyBody, BlockHeader};
use alloy_eips::{
    eip4844::DATA_GAS_PER_BLOB, eip7594::CELLS_PER_EXT_BLOB, eip7685::RequestsOrHash,
};
use alloy_primitives::{Bloom, Bytes, Sealable, U256};
use alloy_rpc_types_engine::{
    ClientVersionV1, ExecutionData, ExecutionPayload, ExecutionPayloadEnvelopeV2,
    ExecutionPayloadEnvelopeV3, ExecutionPayloadEnvelopeV4, ExecutionPayloadEnvelopeV5,
    ExecutionPayloadEnvelopeV6, ExecutionPayloadV1, PayloadAttributes, PayloadError,
};
use jsonrpsee_core::server::RpcModule;
use pureth_withdrawal_root::WithdrawalSnapshot;
use reth_chainspec::{
    ChainSpec, EthChainSpec, EthereumHardfork, EthereumHardforks, ForkCondition, Hardforks,
};
use reth_engine_primitives::{ConsensusEngineHandle, EngineApiValidator, PayloadValidator};
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_ethereum_payload_builder::EthereumExecutionPayloadValidator;
use reth_ethereum_primitives::TransactionSigned;
use reth_network_api::NetworkInfo;
use reth_payload_builder::PayloadStore;
use reth_payload_primitives::{
    validate_execution_requests, validate_version_specific_fields, EngineApiMessageVersion,
    EngineObjectValidationError, NewPayloadError, PayloadOrAttributes, PayloadTypes,
};
use reth_payload_validator::{amsterdam, cancun, prague, shanghai};
use reth_primitives_traits::{Block, BlockBody, SealedBlock};
use reth_pureth_block::{
    checked_blob_fee, BlobFeesPerGas, BlockAccessListSnapshot, ExecutionBlock, ExecutionBody,
    ExecutionForkConfig, ExecutionHeader, ExecutionRequests, ExecutionRequestsSnapshot,
    ExecutionSummary, GasAmounts, PayloadLimits, PayloadSourceError, PayloadSourceInput,
};
use reth_pureth_transaction::{convert_transactions, TransactionSnapshot};
use reth_rpc_api::EngineApiServer;
use reth_rpc_engine_api::{
    capabilities::{EngineCapabilities, CAPABILITIES},
    EngineApi,
};
use reth_storage_api::{BalProvider, BlockReader, HeaderProvider, StateProviderFactory};
use reth_tasks::Runtime;
use reth_transaction_pool::TransactionPool;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub type ExecutionEngineTypes = EthEngineTypes<ExecutionPayloadTypes>;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ExecutionPayloadTypes;

impl PayloadTypes for ExecutionPayloadTypes {
    type BuiltPayload = ExecutionBuiltPayload;
    type PayloadAttributes = PayloadAttributes;
    type ExecutionData = ExecutionData;

    fn block_to_payload(
        block: SealedBlock<ExecutionBlock>,
        bal: Option<Bytes>,
    ) -> Result<ExecutionData, NewPayloadError> {
        if block.header().is_ssz() {
            return Err(NewPayloadError::other(PayloadSourceError::MissingInput(
                PayloadSourceInput::Requests,
            )));
        }
        let (payload, sidecar) = ExecutionPayload::from_block_unchecked_with_extras(
            block.hash(),
            &block.into_block().into_ethereum_block(),
            bal,
        );
        Ok(ExecutionData::new(payload, sidecar))
    }
}

impl From<ExecutionBuiltPayload> for ExecutionData {
    fn from(value: ExecutionBuiltPayload) -> Self {
        value.inner.into_execution_data()
    }
}

macro_rules! legacy_envelopes {
    ($($envelope:ty),* $(,)?) => {
        $(impl TryFrom<ExecutionBuiltPayload> for $envelope {
            type Error = EngineObjectValidationError;

            fn try_from(value: ExecutionBuiltPayload) -> Result<Self, Self::Error> {
                if value.inner.block().header().is_ssz() {
                    return Err(EngineObjectValidationError::UnsupportedFork);
                }
                value.inner.try_into().map_err(EngineObjectValidationError::invalid_params)
            }
        })*
    };
}

legacy_envelopes!(
    ExecutionPayloadV1,
    ExecutionPayloadEnvelopeV2,
    ExecutionPayloadEnvelopeV3,
    ExecutionPayloadEnvelopeV4,
    ExecutionPayloadEnvelopeV5
);

impl TryFrom<ExecutionBuiltPayload> for ExecutionPayloadEnvelopeV6 {
    type Error = EngineObjectValidationError;

    fn try_from(value: ExecutionBuiltPayload) -> Result<Self, Self::Error> {
        if !value.inner.block().header().is_ssz() {
            return Err(EngineObjectValidationError::UnsupportedFork);
        }
        let hashes =
            value.inner.block().body().blob_versioned_hashes_iter().copied().collect::<Vec<_>>();
        let envelope =
            value.inner.try_into_v6().map_err(EngineObjectValidationError::invalid_params)?;
        let bundle = &envelope.blobs_bundle;
        if bundle.versioned_hashes() != hashes ||
            bundle.commitments.len() != bundle.blobs.len() ||
            bundle.blobs.len().checked_mul(CELLS_PER_EXT_BLOB) != Some(bundle.proofs.len())
        {
            return Err(EngineObjectValidationError::invalid_params(
                PayloadSourceError::SourceMismatch,
            ));
        }
        Ok(envelope)
    }
}

#[derive(Clone, Debug)]
pub struct ExecutionEngineValidator {
    chain_spec: Arc<ChainSpec>,
    fork: ExecutionForkConfig,
    limits: PayloadLimits,
}

impl ExecutionEngineValidator {
    pub fn new(
        chain_spec: Arc<ChainSpec>,
        limits: PayloadLimits,
    ) -> Result<Self, PayloadSourceError> {
        let timestamp =
            chain_spec.genesis.config.amsterdam_time.ok_or(PayloadSourceError::UnsupportedFork)?;
        if chain_spec.fork(EthereumHardfork::Amsterdam) != ForkCondition::Timestamp(timestamp) ||
            chain_spec.fork(EthereumHardfork::Bogota) != ForkCondition::Never ||
            chain_spec.genesis.config.bogota_time.is_some() ||
            !chain_spec.is_shanghai_active_at_timestamp(timestamp) ||
            !chain_spec.is_cancun_active_at_timestamp(timestamp) ||
            !chain_spec.is_prague_active_at_timestamp(timestamp) ||
            !chain_spec.is_osaka_active_at_timestamp(timestamp)
        {
            return Err(PayloadSourceError::UnsupportedFork);
        }
        if chain_spec.blob_params_at_timestamp(timestamp).is_none() ||
            chain_spec
                .blob_params
                .scheduled
                .windows(2)
                .any(|entries| entries[0].0 >= entries[1].0) ||
            chain_spec
                .blob_params
                .scheduled
                .iter()
                .any(|(timestamp, _)| !chain_spec.is_osaka_active_at_timestamp(*timestamp))
        {
            return Err(PayloadSourceError::InvalidBlobParameters);
        }
        for params in [
            &chain_spec.blob_params.cancun,
            &chain_spec.blob_params.prague,
            &chain_spec.blob_params.osaka,
        ]
        .into_iter()
        .chain(chain_spec.blob_params.scheduled.iter().map(|(_, params)| params))
        {
            if params.target_blob_count == 0 ||
                params.max_blob_count < params.target_blob_count ||
                params.max_blob_count.checked_mul(DATA_GAS_PER_BLOB).is_none() ||
                params.update_fraction == 0 ||
                params.min_blob_fee == 0
            {
                return Err(PayloadSourceError::InvalidBlobParameters);
            }
        }
        Ok(Self { chain_spec, fork: ExecutionForkConfig::new(timestamp), limits })
    }

    pub const fn fork_config(&self) -> ExecutionForkConfig {
        self.fork
    }

    pub const fn limits(&self) -> PayloadLimits {
        self.limits
    }

    pub const fn chain_spec(&self) -> &Arc<ChainSpec> {
        &self.chain_spec
    }

    #[allow(clippy::too_many_arguments)]
    pub fn into_rpc<Provider, Pool>(
        self,
        provider: Provider,
        consensus: ConsensusEngineHandle<ExecutionEngineTypes>,
        payloads: PayloadStore<ExecutionEngineTypes>,
        pool: Pool,
        runtime: Runtime,
        client: ClientVersionV1,
        network: impl NetworkInfo + 'static,
    ) -> RpcModule<()>
    where
        Provider: HeaderProvider
            + BlockReader<Block = ExecutionBlock>
            + StateProviderFactory
            + BalProvider
            + 'static,
        Pool: TransactionPool + 'static,
    {
        let excluded = [
            "engine_forkchoiceUpdatedV5",
            "engine_newPayloadV6",
            "engine_getInclusionListV1",
            "engine_getPayloadBodiesByHashV1",
            "engine_getPayloadBodiesByHashV2",
            "engine_getPayloadBodiesByRangeV1",
            "engine_getPayloadBodiesByRangeV2",
        ];
        let capabilities = EngineCapabilities::new(
            CAPABILITIES.iter().copied().filter(|method| !excluded.contains(method)),
        );
        let mut module = EngineApi::new(
            provider,
            self.chain_spec.clone(),
            consensus,
            payloads,
            pool,
            runtime,
            client,
            capabilities,
            self,
            false,
            network,
        )
        .into_rpc()
        .remove_context();
        for method in excluded {
            module.remove_method(method);
        }
        module
    }

    fn active_block(
        &self,
        data: ExecutionData,
    ) -> Result<SealedBlock<ExecutionBlock>, NewPayloadError> {
        if data.sidecar.bogota().is_some() {
            return Err(NewPayloadError::other(PayloadSourceError::UnsupportedFork));
        }
        let input = data.payload.as_v1();
        let size = input
            .transactions
            .iter()
            .chain(data.sidecar.requests().map(|requests| requests.iter()).into_iter().flatten())
            .try_fold(input.extra_data.len(), |size, bytes| size.checked_add(bytes.len()))
            .and_then(|size| {
                size.checked_add(data.payload.block_access_list().map_or(0, |bytes| bytes.len()))
            })
            .and_then(|size| {
                data.payload.withdrawals().map_or(Some(size), |values| {
                    values.len().checked_mul(44).and_then(|bytes| size.checked_add(bytes))
                })
            });
        if input.transactions.len() > self.limits.transactions ||
            data.payload.block_access_list().is_some_and(|bal| bal.len() > self.limits.bal_bytes) ||
            data.payload
                .withdrawals()
                .is_some_and(|values| values.len() > self.limits.withdrawals) ||
            size.is_none_or(|size| size > self.limits.bytes)
        {
            return Err(NewPayloadError::Unavailable(Box::new(PayloadSourceError::LimitExceeded)));
        }
        if reth_tasks::cancel::is_cancelled() {
            return Err(NewPayloadError::Unavailable(Box::new(PayloadSourceError::Cancelled)));
        }
        let bal = data
            .payload
            .block_access_list()
            .ok_or(PayloadError::PostAmsterdamBlockWithoutBlockAccessList)?;
        let bal = BlockAccessListSnapshot::from_rlp(bal.clone(), self.limits.bal_bytes)
            .map_err(|error| NewPayloadError::InvalidBlockAccessList(Box::new(error)))?;
        let requests = data.sidecar.requests().ok_or_else(|| {
            NewPayloadError::other(PayloadSourceError::MissingInput(PayloadSourceInput::Requests))
        })?;
        let requests = ExecutionRequestsSnapshot::build(
            ExecutionRequests::from_native_groups(requests)
                .map_err(|error| NewPayloadError::other(PayloadSourceError::Requests(error)))?,
        )
        .map_err(|error| NewPayloadError::other(PayloadSourceError::Requests(error)))?;
        let expected = data.payload.block_hash();
        let block = data.payload.try_into_block_with_sidecar::<TransactionSigned>(&data.sidecar)?;
        if block.header.logs_bloom != Bloom::repeat_byte(0xff) {
            return Err(NewPayloadError::other(PayloadSourceError::SourceMismatch));
        }
        let native = block.seal_slow();
        shanghai::ensure_well_formed_fields(native.body(), true)?;
        cancun::ensure_well_formed_header_and_sidecar_fields(&native, data.sidecar.cancun(), true)?;
        prague::ensure_well_formed_fields(native.body(), data.sidecar.prague(), true)?;
        amsterdam::ensure_well_formed_fields(&native, true)?;
        let params = self
            .chain_spec
            .blob_params_at_timestamp(native.timestamp())
            .ok_or_else(|| NewPayloadError::other(PayloadSourceError::InvalidBlobParameters))?;
        let blob_limit = params
            .max_blob_count
            .checked_mul(DATA_GAS_PER_BLOB)
            .ok_or_else(|| NewPayloadError::other(PayloadSourceError::InvalidBlobParameters))?;
        let header = native.header();
        let blob_gas_used =
            header.blob_gas_used.ok_or(PayloadError::PostCancunBlockWithoutBlobGasUsed)?;
        let excess_blob_gas =
            header.excess_blob_gas.ok_or(PayloadError::PostCancunBlockWithoutExcessBlobGas)?;
        let blob_fee =
            checked_blob_fee(&params, excess_blob_gas).map_err(NewPayloadError::other)?;
        let transactions = TransactionSnapshot::build(
            convert_transactions(&native.body().transactions).map_err(NewPayloadError::other)?,
        )
        .map_err(NewPayloadError::other)?;
        let withdrawals = WithdrawalSnapshot::build(
            native
                .body()
                .withdrawals
                .as_ref()
                .ok_or(PayloadError::PostShanghaiBlockWithoutWithdrawals)?
                .to_vec(),
        )
        .map_err(NewPayloadError::other)?;
        let header = ExecutionHeader::from_summary(ExecutionSummary {
            parent_hash: header.parent_hash,
            miner: header.beneficiary,
            state_root: header.state_root,
            transactions_root: transactions.root(),
            receipts_root: header.receipts_root,
            number: header.number,
            gas_limits: GasAmounts { regular: header.gas_limit, blob: blob_limit },
            gas_used: GasAmounts { regular: header.gas_used, blob: blob_gas_used },
            timestamp: header.timestamp,
            extra_data: header.extra_data.clone(),
            mix_hash: header.mix_hash,
            base_fees_per_gas: BlobFeesPerGas {
                regular: U256::from(header.base_fee_per_gas.ok_or_else(|| {
                    NewPayloadError::other(PayloadSourceError::MissingInput(
                        PayloadSourceInput::BaseFee,
                    ))
                })?),
                blob: blob_fee,
            },
            withdrawals_root: withdrawals.root(),
            excess_gas: GasAmounts { regular: 0, blob: excess_blob_gas },
            parent_beacon_block_root: header
                .parent_beacon_block_root
                .ok_or(PayloadError::PostCancunBlockWithoutParentBeaconBlockRoot)?,
            requests_root: requests.root(),
            block_access_list_root: bal.root(),
            slot_number: header
                .slot_number
                .ok_or(PayloadError::PostAmsterdamBlockWithoutSlotNumber)?,
        })
        .map_err(NewPayloadError::other)?;
        if header.hash_slow() != expected {
            return Err(PayloadError::BlockHash {
                execution: header.hash_slow(),
                consensus: expected,
            }
            .into());
        }
        cancun::ensure_well_formed_transactions_field_with_sidecar(
            native.body(),
            data.sidecar.cancun(),
            true,
        )?;
        let body = native.into_block().body;
        let body = ExecutionBody::from_header(
            &header,
            AlloyBody {
                transactions: body.transactions,
                ommers: Vec::new(),
                withdrawals: body.withdrawals,
            },
        )
        .map_err(NewPayloadError::other)?;
        let block = ExecutionBlock::try_new(header, body).map_err(NewPayloadError::other)?;
        Ok(block.seal_slow())
    }
}

impl PayloadValidator<ExecutionEngineTypes> for ExecutionEngineValidator {
    type Block = ExecutionBlock;

    fn convert_payload_to_block(
        &self,
        data: ExecutionData,
    ) -> Result<SealedBlock<ExecutionBlock>, NewPayloadError> {
        if self.fork.is_active_at_timestamp(data.payload.timestamp()) {
            return self.active_block(data);
        }
        let block = EthereumExecutionPayloadValidator::new(self.chain_spec.clone())
            .ensure_well_formed_payload::<TransactionSigned>(data)?;
        let block = block.into_block();
        let header = ExecutionHeader::legacy(block.header);
        let body = ExecutionBody::from_header(
            &header,
            AlloyBody {
                transactions: block.body.transactions,
                ommers: block.body.ommers.into_iter().map(ExecutionHeader::legacy).collect(),
                withdrawals: block.body.withdrawals,
            },
        )
        .map_err(NewPayloadError::other)?;
        Ok(ExecutionBlock::try_new(header, body).map_err(NewPayloadError::other)?.seal_slow())
    }
}

impl EngineApiValidator<ExecutionEngineTypes> for ExecutionEngineValidator {
    fn validate_version_specific_fields(
        &self,
        version: EngineApiMessageVersion,
        value: PayloadOrAttributes<'_, ExecutionData, PayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        if self.fork.is_active_at_timestamp(value.timestamp()) &&
            version !=
                match value {
                    PayloadOrAttributes::ExecutionPayload(_) => EngineApiMessageVersion::V5,
                    PayloadOrAttributes::PayloadAttributes(_) => EngineApiMessageVersion::V4,
                }
        {
            return Err(EngineObjectValidationError::UnsupportedFork);
        }
        if let PayloadOrAttributes::ExecutionPayload(data) = &value &&
            let Some(prague) = data.sidecar.prague()
        {
            self.validate_payload_requests(data.payload.timestamp(), &prague.requests)?;
        }
        if let Some(requests) = value.execution_requests() {
            validate_execution_requests(requests)?;
            if self.fork.is_active_at_timestamp(value.timestamp()) {
                ExecutionRequests::from_native_groups(requests)
                    .map_err(EngineObjectValidationError::invalid_params)?;
            }
        }
        validate_version_specific_fields(self.chain_spec.as_ref(), version, value)
    }

    fn ensure_well_formed_attributes(
        &self,
        version: EngineApiMessageVersion,
        attributes: &PayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        if self.fork.is_active_at_timestamp(attributes.timestamp) &&
            version != EngineApiMessageVersion::V4
        {
            return Err(EngineObjectValidationError::UnsupportedFork);
        }
        validate_version_specific_fields(
            self.chain_spec.as_ref(),
            version,
            PayloadOrAttributes::<ExecutionData, PayloadAttributes>::from_attributes(attributes),
        )?;
        if self.fork.is_active_at_timestamp(attributes.timestamp) &&
            attributes.target_gas_limit.is_none()
        {
            return Err(EngineObjectValidationError::PayloadAttributes(
                reth_payload_primitives::VersionSpecificValidationError::NoTargetGasLimitPostAmsterdam));
        }
        Ok(())
    }

    fn validate_payload_requests(
        &self,
        timestamp: u64,
        requests: &RequestsOrHash,
    ) -> Result<(), EngineObjectValidationError> {
        if self.fork.is_active_at_timestamp(timestamp) && requests.is_hash() {
            return Err(EngineObjectValidationError::invalid_params(
                PayloadSourceError::MissingInput(PayloadSourceInput::Requests),
            ));
        }
        Ok(())
    }

    fn validate_get_payload_version(
        &self,
        version: EngineApiMessageVersion,
        timestamp: u64,
    ) -> Result<(), EngineObjectValidationError> {
        if self.fork.is_active_at_timestamp(timestamp) && version != EngineApiMessageVersion::V6 {
            return Err(EngineObjectValidationError::UnsupportedFork);
        }
        Ok(())
    }
}
