//! Execution witness generation and temporary Bogota SSZ Engine API wire types.

use alloy_eips::{eip4895::Withdrawal, eip7685::Requests};
use alloy_primitives::{Address, Bytes, B128, B256};
use alloy_rpc_types_engine::{
    ssz_engine_types::{
        BuiltPayloadAmsterdam, ConversionError, ExecutionPayloadAmsterdam,
        ExecutionPayloadBodyAmsterdam, Optional, PayloadAttributesAmsterdam,
        PayloadAttributesConversionError, PayloadStatus, PayloadStatusKind, ValidationError,
    },
    ExecutionData, ForkchoiceState, ForkchoiceUpdatedResponseV2,
    PayloadAttributes as LegacyPayloadAttributes, PayloadId, PayloadStatusV2,
};
use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::{AlloyBlockHeader, Block};
use reth_provider::{HeaderProvider, StateProvider, StateProviderBox, StateProviderFactory};
use reth_revm::{database::StateProviderDatabase, witness::ExecutionWitnessRecord};
use reth_tasks::Runtime;
use reth_trie_common::ExecutionWitnessMode;
use std::{future::Future, pin::Pin};

/// Re-executes validated payloads against their parent state for `/payloads/witness`.
///
/// The parent state must be available through the provider (persisted, canonical in-memory,
/// or pending). Parents present only in the engine tree are reported as
/// [`EngineSszWitnessError::ParentStateUnavailable`]; the route then answers with the payload
/// status alone.
#[derive(Clone, Debug)]
pub struct EngineSszWitnessGenerator<Provider, Evm> {
    provider: Provider,
    evm_config: Evm,
    task_spawner: Runtime,
}

impl<Provider, Evm> EngineSszWitnessGenerator<Provider, Evm> {
    /// Creates a new witness generator.
    pub const fn new(provider: Provider, evm_config: Evm, task_spawner: Runtime) -> Self {
        Self { provider, evm_config, task_spawner }
    }
}

impl<Provider, Evm> EngineSszWitness for EngineSszWitnessGenerator<Provider, Evm>
where
    Provider: HeaderProvider + StateProviderFactory + Clone + Send + Sync + 'static,
    Provider::Header: alloy_rlp::Encodable,
    Evm: ConfigureEvm<Primitives = EthPrimitives> + 'static,
{
    fn generate_witness(
        &self,
        payload: ExecutionData,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ExecutionWitnessV1, EngineSszWitnessError>> + Send + 'static,
        >,
    > {
        let provider = self.provider.clone();
        let evm_config = self.evm_config.clone();
        let task_spawner = self.task_spawner.clone();

        Box::pin(async move {
            task_spawner
                .spawn_blocking(move || {
                    // A VALID newPayload need not be canonical or visible through the provider.
                    let block = payload
                        .payload
                        .try_into_block_with_sidecar::<TransactionSigned>(&payload.sidecar)
                        .map_err(eyre::Report::new)?
                        .try_into_recovered()
                        .map_err(eyre::Report::new)?;

                    let block_number = block.header().number;
                    let parent_hash = block.header().parent_hash;
                    let state_provider =
                        provider.state_by_block_hash(parent_hash).map_err(|source| {
                            EngineSszWitnessError::ParentStateUnavailable {
                                parent: parent_hash,
                                source: eyre::Report::new(source),
                            }
                        })?;
                    let block_executor = evm_config.executor(StateProviderDatabase::new(
                        state_provider.into_evm_state_provider(),
                    ));
                    let mut witness = None;
                    let mut first_header = block_number.saturating_sub(1);
                    block_executor
                        .execute_with_state_closure(&block, |statedb: &reth_revm::State<_>| {
                            if let Some((number, _)) = statedb.block_hashes.lowest() {
                                first_header = number;
                            }
                            witness = Some(
                                ExecutionWitnessRecord::new(statedb)
                                    .into_execution_witness_without_headers::<StateProviderBox>(
                                        &statedb.database,
                                        ExecutionWitnessMode::Canonical,
                                    ),
                            );
                        })
                        .map_err(eyre::Report::new)?;

                    let witness = witness
                        .expect("state closure is called after successful execution")
                        .map_err(eyre::Report::new)?;

                    // Header numbers may refer to a different canonical ancestor.
                    let mut headers = Vec::new();
                    let mut hash = parent_hash;
                    for _ in first_header..block_number {
                        let header = provider
                            .header(hash)
                            .map_err(eyre::Report::new)?
                            .ok_or_else(|| eyre::eyre!("ancestor {hash} not found for witness"))?;
                        hash = header.parent_hash();
                        headers.push(alloy_rlp::encode(&header).into());
                    }
                    headers.reverse();

                    Ok(ExecutionWitnessV1 { state: witness.state, codes: witness.codes, headers })
                })
                .await
                .map_err(eyre::Report::new)?
        })
    }
}

/// Generates an execution witness for a valid payload.
pub trait EngineSszWitness: Send + Sync + 'static {
    /// Generates a REST-SSZ execution witness after the submitted payload has been validated.
    fn generate_witness(
        &self,
        payload: ExecutionData,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ExecutionWitnessV1, EngineSszWitnessError>> + Send + 'static,
        >,
    >;
}

/// Failure to produce a witness for a validated payload.
#[derive(Debug)]
pub enum EngineSszWitnessError {
    /// The parent state is not available through the provider yet.
    ParentStateUnavailable {
        /// Parent block whose state is required.
        parent: B256,
        /// Provider failure while accessing the state.
        source: eyre::Report,
    },
    /// Witness execution or proof generation failed.
    Internal(eyre::Report),
}

impl From<eyre::Report> for EngineSszWitnessError {
    fn from(error: eyre::Report) -> Self {
        Self::Internal(error)
    }
}

impl std::fmt::Display for EngineSszWitnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParentStateUnavailable { parent, source } => {
                write!(f, "parent state {parent} is unavailable through the provider: {source}")
            }
            Self::Internal(error) => std::fmt::Display::fmt(error, f),
        }
    }
}

impl std::error::Error for EngineSszWitnessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ParentStateUnavailable { source, .. } | Self::Internal(source) => {
                Some(source.as_ref())
            }
        }
    }
}

/// A trie-node byte list in an [`ExecutionWitnessV1`].
pub type WitnessNodeV1 = Bytes;

/// A contract-code byte list in an [`ExecutionWitnessV1`].
pub type WitnessCodeV1 = Bytes;

/// An RLP-encoded header byte list in an [`ExecutionWitnessV1`].
pub type WitnessHeaderV1 = Bytes;

/// Canonical execution witness for `POST /payloads/witness`.
///
/// `state` and `codes` are produced in lexicographic ascending byte order. `headers` are
/// RLP-encoded and ordered by ascending block number; consecutive headers must be parent-linked.
/// These ordering rules are producer-side requirements from the execution-specs witness builder.
///
/// This is a REST-SSZ wire container, not the JSON-RPC debug witness shape.
#[derive(Clone, Debug, Default, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ExecutionWitnessV1 {
    /// Hashed trie-node preimages required during execution and state-root recomputation.
    pub state: Vec<WitnessNodeV1>,
    /// Contract bytecode preimages required from the pre-state.
    pub codes: Vec<WitnessCodeV1>,
    /// RLP-encoded ancestor headers used for pre-state and `BLOCKHASH` correctness proofs.
    pub headers: Vec<WitnessHeaderV1>,
}

/// Canonical execution witness for `POST /payloads/witness`.
pub type ExecutionWitness = ExecutionWitnessV1;

/// REST-SSZ response for `POST /payloads/witness`.
///
/// The witness uses the Engine REST-SSZ `Optional[T]` encoding from execution-apis and is present
/// only when the payload status is `VALID`. A `VALID` status without a witness means the parent
/// state was not yet available through the provider (the parent is only known to the engine
/// tree); resubmitting the payload once forkchoice has made the parent canonical yields it.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode)]
pub struct PayloadStatusWithWitness {
    /// Result of processing the submitted payload.
    pub payload_status: PayloadStatus,
    /// Execution witness produced for a valid payload.
    pub witness: Optional<ExecutionWitnessV1>,
}

impl PayloadStatusWithWitness {
    /// Creates a response, converting the witness into the REST-SSZ `Optional[T]` representation.
    pub fn new(payload_status: PayloadStatus, witness: Option<ExecutionWitnessV1>) -> Self {
        let witness = match payload_status.status {
            PayloadStatusKind::Valid => witness.into(),
            _ => Optional::none(),
        };
        Self { payload_status, witness }
    }
}

/// Backwards-compatible alias for the experimental witness response name.
pub type NewPayloadWithWitnessResponseV1 = PayloadStatusWithWitness;

impl ssz::Decode for PayloadStatusWithWitness {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        let mut builder = ssz::SszDecoderBuilder::new(bytes);
        builder.register_type::<PayloadStatus>()?;
        builder.register_type::<Optional<ExecutionWitnessV1>>()?;
        let mut decoder = builder.build()?;
        let response =
            Self { payload_status: decoder.decode_next()?, witness: decoder.decode_next()? };
        if response.witness.is_some() && response.payload_status.status != PayloadStatusKind::Valid
        {
            return Err(ssz::DecodeError::BytesInvalid(
                "execution witness is only valid for VALID payload status".into(),
            ))
        }
        Ok(response)
    }
}

/// Bogota execution payload, with the Amsterdam wire shape.
pub type ExecutionPayloadBogota = ExecutionPayloadAmsterdam;

/// Bogota execution payload body, with the Amsterdam wire shape.
pub type ExecutionPayloadBodyBogota = ExecutionPayloadBodyAmsterdam;

/// Bogota built payload, with the Amsterdam wire shape.
pub type BuiltPayloadBogota = BuiltPayloadAmsterdam;

/// Maximum cumulative transaction byte length returned by `/inclusion-list`.
pub const MAX_TRANSACTIONS_BYTES_PER_INCLUSION_LIST: usize = 1 << 13;

/// Bogota payload attributes, with inclusion-list transactions appended to Amsterdam fields.
#[derive(Clone, Debug, Default, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct PayloadAttributesBogota {
    /// Payload timestamp.
    pub timestamp: u64,
    /// Previous RANDAO value.
    pub prev_randao: B256,
    /// Suggested fee recipient.
    pub suggested_fee_recipient: Address,
    /// Withdrawals to include in the payload.
    pub withdrawals: Vec<Withdrawal>,
    /// Root of the parent beacon block.
    pub parent_beacon_block_root: B256,
    /// Consensus-layer slot number.
    pub slot_number: u64,
    /// Target gas limit.
    pub target_gas_limit: u64,
    /// Transactions used to enforce the EIP-7805 inclusion-list constraints.
    pub inclusion_list_transactions: Vec<Bytes>,
}

impl PayloadAttributesBogota {
    /// Separates the Amsterdam fields and inclusion list without discarding either.
    pub fn into_parts(self) -> (PayloadAttributesAmsterdam, Vec<Bytes>) {
        (
            PayloadAttributesAmsterdam {
                timestamp: self.timestamp,
                prev_randao: self.prev_randao,
                suggested_fee_recipient: self.suggested_fee_recipient,
                withdrawals: self.withdrawals,
                parent_beacon_block_root: self.parent_beacon_block_root,
                slot_number: self.slot_number,
                target_gas_limit: self.target_gas_limit,
            },
            self.inclusion_list_transactions,
        )
    }
}

/// Bogota payload-submission request.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ExecutionPayloadEnvelopeBogota {
    /// Submitted execution payload.
    pub payload: ExecutionPayloadBogota,
    /// Root of the parent beacon block.
    pub parent_beacon_block_root: B256,
    /// EIP-7685 execution requests.
    pub execution_requests: Requests,
    /// Transactions used to enforce the EIP-7805 inclusion-list constraints.
    pub inclusion_list_transactions: Vec<Bytes>,
}

/// Bogota forkchoice-update request.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ForkchoiceUpdateBogota {
    /// Current forkchoice state.
    pub forkchoice_state: ForkchoiceState,
    /// Optional Bogota payload attributes.
    pub payload_attributes: Optional<PayloadAttributesBogota>,
    /// Optional `Bitvector[128]` custody-column selection.
    pub custody_columns: Optional<B128>,
}

/// Bogota payload status, with `inclusion_list_satisfied` appended to the common fields.
///
/// The common fields are flattened in SSZ; nesting `PayloadStatus` would change the wire layout.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode)]
pub struct PayloadStatusBogota {
    /// Payload validation status.
    pub status: PayloadStatusKind,
    /// Most recent valid block hash.
    pub latest_valid_hash: Optional<B256>,
    /// Optional payload validation error bytes.
    pub validation_error: Optional<ValidationError>,
    /// Inclusion-list validation result, permitted only for a `VALID` payload.
    pub inclusion_list_satisfied: Optional<bool>,
}

impl ssz::Decode for PayloadStatusBogota {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        let mut builder = ssz::SszDecoderBuilder::new(bytes);
        builder.register_type::<PayloadStatusKind>()?;
        builder.register_type::<Optional<B256>>()?;
        builder.register_type::<Optional<ValidationError>>()?;
        builder.register_type::<Optional<bool>>()?;
        let mut decoder = builder.build()?;
        let response = Self {
            status: decoder.decode_next()?,
            latest_valid_hash: decoder.decode_next()?,
            validation_error: decoder.decode_next()?,
            inclusion_list_satisfied: decoder.decode_next()?,
        };
        if response.status != PayloadStatusKind::Invalid && response.validation_error.is_some() {
            return Err(ssz::DecodeError::BytesInvalid(
                "validation error is only valid for INVALID status".into(),
            ));
        }
        if response.status != PayloadStatusKind::Valid &&
            response.inclusion_list_satisfied.is_some()
        {
            return Err(ssz::DecodeError::BytesInvalid(
                "inclusion list result is only valid for VALID status".into(),
            ));
        }
        Ok(response)
    }
}

/// Error converting Bogota JSON-RPC responses into REST-SSZ containers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BogotaConversionError {
    /// A common status field is invalid.
    Status(ConversionError),
    /// An inclusion-list result was supplied for a non-valid payload.
    InvalidInclusionListStatus,
}

impl core::fmt::Display for BogotaConversionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Status(error) => core::fmt::Display::fmt(error, f),
            Self::InvalidInclusionListStatus => {
                f.write_str("inclusion list result is only valid for VALID status")
            }
        }
    }
}

impl core::error::Error for BogotaConversionError {}

impl TryFrom<PayloadStatusV2> for PayloadStatusBogota {
    type Error = BogotaConversionError;

    fn try_from(value: PayloadStatusV2) -> Result<Self, Self::Error> {
        let status =
            PayloadStatus::try_from(value.payload_inner).map_err(BogotaConversionError::Status)?;
        if status.status != PayloadStatusKind::Valid && value.inclusion_list_satisfied.is_some() {
            return Err(BogotaConversionError::InvalidInclusionListStatus);
        }
        Ok(Self {
            status: status.status,
            latest_valid_hash: status.latest_valid_hash,
            validation_error: status.validation_error,
            inclusion_list_satisfied: value.inclusion_list_satisfied.into(),
        })
    }
}

impl From<PayloadStatusBogota> for PayloadStatusV2 {
    fn from(value: PayloadStatusBogota) -> Self {
        Self {
            payload_inner: PayloadStatus {
                status: value.status,
                latest_valid_hash: value.latest_valid_hash,
                validation_error: value.validation_error,
            }
            .into(),
            inclusion_list_satisfied: value.inclusion_list_satisfied.into_option(),
        }
    }
}

/// Bogota forkchoice response, carrying the extended payload status.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode)]
pub struct ForkchoiceUpdateResponseBogota {
    /// Restricted payload status; `ACCEPTED` is invalid here.
    pub payload_status: PayloadStatusBogota,
    /// Opaque server-assigned payload identifier.
    pub payload_id: Optional<PayloadId>,
}

impl ssz::Decode for ForkchoiceUpdateResponseBogota {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        let mut builder = ssz::SszDecoderBuilder::new(bytes);
        builder.register_type::<PayloadStatusBogota>()?;
        builder.register_type::<Optional<PayloadId>>()?;
        let mut decoder = builder.build()?;
        let response =
            Self { payload_status: decoder.decode_next()?, payload_id: decoder.decode_next()? };
        if response.payload_status.status == PayloadStatusKind::Accepted {
            return Err(ssz::DecodeError::BytesInvalid(
                "ACCEPTED is not valid in a forkchoice response".into(),
            ));
        }
        Ok(response)
    }
}

impl TryFrom<ForkchoiceUpdatedResponseV2> for ForkchoiceUpdateResponseBogota {
    type Error = BogotaConversionError;

    fn try_from(value: ForkchoiceUpdatedResponseV2) -> Result<Self, Self::Error> {
        let payload_status = PayloadStatusBogota::try_from(value.payload_status)?;
        if payload_status.status == PayloadStatusKind::Accepted {
            return Err(BogotaConversionError::Status(ConversionError::AcceptedForkchoice));
        }
        Ok(Self { payload_status, payload_id: value.payload_id.into() })
    }
}

impl From<ForkchoiceUpdateResponseBogota> for ForkchoiceUpdatedResponseV2 {
    fn from(value: ForkchoiceUpdateResponseBogota) -> Self {
        Self {
            payload_status: value.payload_status.into(),
            payload_id: value.payload_id.into_option(),
        }
    }
}

/// REST-SSZ response for `GET /inclusion-list`.
#[derive(Clone, Debug, Default, PartialEq, Eq, ssz_derive::Encode)]
pub struct InclusionListResponse {
    /// Non-empty, non-blob transactions totaling at most 8,192 bytes.
    pub transactions: Vec<Bytes>,
}

impl InclusionListResponse {
    /// Checks the transaction constraints required by the inclusion-list endpoint.
    pub fn validate(&self) -> Result<(), ssz::DecodeError> {
        let mut total = 0;
        for transaction in &self.transactions {
            if transaction.is_empty() || transaction[0] == 3 {
                return Err(ssz::DecodeError::BytesInvalid(
                    "inclusion list transactions must be non-empty and must not be blobs".into(),
                ));
            }
            // Check each addition against the remaining budget before adding, avoiding overflow.
            if transaction.len() > MAX_TRANSACTIONS_BYTES_PER_INCLUSION_LIST - total {
                return Err(ssz::DecodeError::BytesInvalid(
                    "inclusion list exceeds 8192 transaction bytes".into(),
                ));
            }
            total += transaction.len();
        }
        Ok(())
    }
}

impl ssz::Decode for InclusionListResponse {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        let mut builder = ssz::SszDecoderBuilder::new(bytes);
        builder.register_type::<Vec<Bytes>>()?;
        let mut decoder = builder.build()?;
        let response = Self { transactions: decoder.decode_next()? };
        response.validate()?;
        Ok(response)
    }
}

impl From<PayloadAttributesBogota> for LegacyPayloadAttributes {
    fn from(value: PayloadAttributesBogota) -> Self {
        let (attributes, transactions) = value.into_parts();
        Self::from(attributes).with_inclusion_list_transactions(transactions)
    }
}

impl TryFrom<LegacyPayloadAttributes> for PayloadAttributesBogota {
    type Error = PayloadAttributesConversionError;

    fn try_from(mut value: LegacyPayloadAttributes) -> Result<Self, Self::Error> {
        let inclusion_list_transactions = value
            .inclusion_list_transactions
            .take()
            .ok_or(PayloadAttributesConversionError::MissingField("inclusion_list_transactions"))?;
        let value = PayloadAttributesAmsterdam::try_from(value)?;
        Ok(Self {
            timestamp: value.timestamp,
            prev_randao: value.prev_randao,
            suggested_fee_recipient: value.suggested_fee_recipient,
            withdrawals: value.withdrawals,
            parent_beacon_block_root: value.parent_beacon_block_root,
            slot_number: value.slot_number,
            target_gas_limit: value.target_gas_limit,
            inclusion_list_transactions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssz::{Decode, Encode};

    fn assert_roundtrip<T>(value: &T)
    where
        T: Encode + Decode + PartialEq + core::fmt::Debug,
    {
        assert_eq!(T::from_ssz_bytes(&value.as_ssz_bytes()).unwrap(), *value);
    }

    #[test]
    fn witness_response_roundtrips_when_status_is_valid() {
        let payload_status = PayloadStatus {
            status: PayloadStatusKind::Valid,
            latest_valid_hash: Optional::none(),
            validation_error: Optional::none(),
        };
        let witness = ExecutionWitnessV1 {
            state: vec![vec![1, 2, 3].into()],
            codes: vec![vec![4, 5].into()],
            headers: vec![vec![6].into()],
        };
        let response = PayloadStatusWithWitness::new(payload_status, Some(witness));

        assert_roundtrip(&response);
    }

    #[test]
    fn witness_response_omits_witness_for_non_valid_status() {
        let payload_status = PayloadStatus {
            status: PayloadStatusKind::Syncing,
            latest_valid_hash: Optional::none(),
            validation_error: Optional::none(),
        };
        let response =
            PayloadStatusWithWitness::new(payload_status, Some(ExecutionWitnessV1::default()));

        assert!(response.witness.is_none());
        assert_roundtrip(&response);
    }

    #[test]
    fn bogota_status_has_flat_fields_and_optional_boolean() {
        for satisfied in [None, Some(false), Some(true)] {
            let response = PayloadStatusBogota {
                status: PayloadStatusKind::Valid,
                latest_valid_hash: Optional::none(),
                validation_error: Optional::none(),
                inclusion_list_satisfied: satisfied.into(),
            };
            let mut expected = vec![0, 13, 0, 0, 0, 13, 0, 0, 0, 13, 0, 0, 0];
            if let Some(satisfied) = satisfied {
                expected.push(u8::from(satisfied));
            }
            assert_eq!(response.as_ssz_bytes(), expected);
            assert_eq!(PayloadStatusBogota::from_ssz_bytes(&expected).unwrap(), response);
            let legacy: PayloadStatusV2 = response.clone().into();
            assert_eq!(legacy.inclusion_list_satisfied, satisfied);
            assert_eq!(PayloadStatusBogota::try_from(legacy).unwrap(), response);
        }
    }

    #[test]
    fn bogota_status_rejects_inclusion_result_for_non_valid_payloads() {
        for status in
            [PayloadStatusKind::Invalid, PayloadStatusKind::Syncing, PayloadStatusKind::Accepted]
        {
            for satisfied in [false, true] {
                let response = PayloadStatusBogota {
                    status,
                    latest_valid_hash: Optional::none(),
                    validation_error: Optional::none(),
                    inclusion_list_satisfied: Optional::some(satisfied),
                };
                assert_eq!(
                    PayloadStatusBogota::from_ssz_bytes(&response.as_ssz_bytes()),
                    Err(ssz::DecodeError::BytesInvalid(
                        "inclusion list result is only valid for VALID status".into(),
                    )),
                );
                assert_eq!(
                    PayloadStatusBogota::try_from(PayloadStatusV2::from(response)),
                    Err(BogotaConversionError::InvalidInclusionListStatus),
                );
            }
        }
    }

    #[test]
    fn bogota_status_rejects_invalid_tags_and_error_fields() {
        let response = PayloadStatusBogota {
            status: PayloadStatusKind::Valid,
            latest_valid_hash: Optional::none(),
            validation_error: Optional::some(
                ValidationError::try_from(Bytes::from_static(b"bad")).unwrap(),
            ),
            inclusion_list_satisfied: Optional::none(),
        };
        assert_eq!(
            PayloadStatusBogota::from_ssz_bytes(&response.as_ssz_bytes()),
            Err(ssz::DecodeError::BytesInvalid(
                "validation error is only valid for INVALID status".into(),
            )),
        );
        let mut bytes = vec![4, 13, 0, 0, 0, 13, 0, 0, 0, 13, 0, 0, 0];
        assert!(PayloadStatusBogota::from_ssz_bytes(&bytes).is_err());
        bytes[0] = 0;
        bytes.push(2);
        assert!(PayloadStatusBogota::from_ssz_bytes(&bytes).is_err());
    }

    #[test]
    fn bogota_forkchoice_rejects_accepted_and_preserves_payload_id() {
        for payload_id in [None, Some(PayloadId::new([0; 8]))] {
            let response = ForkchoiceUpdateResponseBogota {
                payload_status: PayloadStatusBogota {
                    status: PayloadStatusKind::Valid,
                    latest_valid_hash: Optional::none(),
                    validation_error: Optional::none(),
                    inclusion_list_satisfied: Optional::some(false),
                },
                payload_id: payload_id.into(),
            };
            assert_eq!(
                ForkchoiceUpdateResponseBogota::from_ssz_bytes(&response.as_ssz_bytes()).unwrap(),
                response
            );
            let legacy: ForkchoiceUpdatedResponseV2 = response.clone().into();
            assert_eq!(ForkchoiceUpdateResponseBogota::try_from(legacy).unwrap(), response);
        }
        let response = ForkchoiceUpdateResponseBogota {
            payload_status: PayloadStatusBogota {
                status: PayloadStatusKind::Accepted,
                latest_valid_hash: Optional::none(),
                validation_error: Optional::none(),
                inclusion_list_satisfied: Optional::none(),
            },
            payload_id: Optional::none(),
        };
        assert_eq!(
            ForkchoiceUpdateResponseBogota::from_ssz_bytes(&response.as_ssz_bytes()),
            Err(ssz::DecodeError::BytesInvalid(
                "ACCEPTED is not valid in a forkchoice response".into()
            ))
        );
        assert_eq!(
            ForkchoiceUpdateResponseBogota::try_from(ForkchoiceUpdatedResponseV2::from(response)),
            Err(BogotaConversionError::Status(ConversionError::AcceptedForkchoice))
        );
    }

    #[test]
    fn inclusion_list_response_is_bounded_single_field_container() {
        assert_eq!(InclusionListResponse::default().as_ssz_bytes(), [4, 0, 0, 0]);
        for lengths in [vec![8192], vec![4096, 4096]] {
            let response = InclusionListResponse {
                transactions: lengths.into_iter().map(|len| Bytes::from(vec![1; len])).collect(),
            };
            assert_eq!(
                InclusionListResponse::from_ssz_bytes(&response.as_ssz_bytes()).unwrap(),
                response
            );
        }
        for transactions in [
            vec![Bytes::new()],
            vec![Bytes::from_static(&[3, 1])],
            vec![Bytes::from(vec![1; 8193])],
            vec![Bytes::from(vec![1; 4096]), Bytes::from(vec![1; 4097])],
        ] {
            let response = InclusionListResponse { transactions };
            assert!(InclusionListResponse::from_ssz_bytes(&response.as_ssz_bytes()).is_err());
        }
        let response = InclusionListResponse { transactions: vec![Bytes::from_static(&[1, 2])] };
        assert_eq!(response.as_ssz_bytes(), [4, 0, 0, 0, 4, 0, 0, 0, 1, 2]);
    }

    #[test]
    fn bogota_attributes_append_inclusion_list_and_preserve_parts() {
        let attributes = PayloadAttributesBogota {
            timestamp: 42,
            slot_number: 10,
            target_gas_limit: 30_000_000,
            inclusion_list_transactions: vec![Bytes::from_static(&[1, 2])],
            ..Default::default()
        };
        let bytes = attributes.as_ssz_bytes();
        assert_eq!(&bytes[60..64], &116u32.to_le_bytes());
        assert_eq!(&bytes[112..116], &116u32.to_le_bytes());
        assert_eq!(PayloadAttributesBogota::from_ssz_bytes(&bytes).unwrap(), attributes);
        let (amsterdam, transactions) = attributes.clone().into_parts();
        assert_eq!(amsterdam.timestamp, attributes.timestamp);
        assert_eq!(amsterdam.slot_number, attributes.slot_number);
        assert_eq!(amsterdam.target_gas_limit, attributes.target_gas_limit);
        assert_eq!(transactions, attributes.inclusion_list_transactions);
        let request = ForkchoiceUpdateBogota {
            forkchoice_state: ForkchoiceState::default(),
            payload_attributes: Optional::some(attributes),
            custody_columns: Optional::some(B128::with_last_byte(1)),
        };
        assert_eq!(
            ForkchoiceUpdateBogota::from_ssz_bytes(&request.as_ssz_bytes()).unwrap(),
            request
        );
    }

    fn check_bogota_decode<T>(bytes: &[u8])
    where
        T: Decode + Encode + PartialEq + core::fmt::Debug,
    {
        if let Ok(value) = T::from_ssz_bytes(bytes) {
            assert_eq!(T::from_ssz_bytes(&value.as_ssz_bytes()).unwrap(), value);
        }
    }

    #[test]
    fn bogota_decoders_handle_mutated_wire_inputs() {
        let seeds = [
            PayloadAttributesBogota::default().as_ssz_bytes(),
            ForkchoiceUpdateBogota {
                forkchoice_state: ForkchoiceState::default(),
                payload_attributes: Optional::some(PayloadAttributesBogota::default()),
                custody_columns: Optional::none(),
            }
            .as_ssz_bytes(),
            PayloadStatusBogota {
                status: PayloadStatusKind::Valid,
                latest_valid_hash: Optional::none(),
                validation_error: Optional::none(),
                inclusion_list_satisfied: Optional::some(false),
            }
            .as_ssz_bytes(),
            InclusionListResponse { transactions: vec![Bytes::from_static(&[1, 2])] }
                .as_ssz_bytes(),
        ];
        for seed in &seeds {
            for len in 0..=seed.len() {
                let bytes = &seed[..len];
                check_bogota_decode::<PayloadAttributesBogota>(bytes);
                check_bogota_decode::<ForkchoiceUpdateBogota>(bytes);
                check_bogota_decode::<PayloadStatusBogota>(bytes);
                check_bogota_decode::<ForkchoiceUpdateResponseBogota>(bytes);
                check_bogota_decode::<InclusionListResponse>(bytes);
            }
            for index in 0..seed.len() {
                for byte in [0, 1, 2, 3, 4, 127, 128, 255] {
                    let mut bytes = seed.clone();
                    bytes[index] = byte;
                    check_bogota_decode::<PayloadAttributesBogota>(&bytes);
                    check_bogota_decode::<ForkchoiceUpdateBogota>(&bytes);
                    check_bogota_decode::<PayloadStatusBogota>(&bytes);
                    check_bogota_decode::<ForkchoiceUpdateResponseBogota>(&bytes);
                    check_bogota_decode::<InclusionListResponse>(&bytes);
                }
            }
        }
    }

    #[test]
    fn bogota_attributes_preserve_build_inclusion_list() {
        let attributes = PayloadAttributesBogota {
            inclusion_list_transactions: vec![Bytes::from_static(&[1, 2])],
            ..Default::default()
        };
        let legacy = LegacyPayloadAttributes::from(attributes.clone());
        assert_eq!(
            legacy.inclusion_list_transactions,
            Some(attributes.inclusion_list_transactions.clone())
        );
        assert_eq!(PayloadAttributesBogota::try_from(legacy).unwrap(), attributes);
    }
}
