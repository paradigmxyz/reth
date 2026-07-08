//! Experimental Engine API v2 REST-SSZ wire types.
//!
//! These types intentionally live apart from the legacy JSON-RPC Engine API types because their
//! SSZ encodings are not always wire-compatible. This module contains the shared endpoint
//! containers, fork-specific payload containers, blob containers, payload-body containers, and the
//! experimental payload-with-witness response type from the same REST-SSZ model.

use alloy_eips::{
    eip4844::{Blob, BlobAndProofV1, BlobAndProofV2, BlobCellsAndProofsV1, Bytes48},
    eip4895::Withdrawal,
    eip7594::{Cell, CELLS_PER_EXT_BLOB},
    eip7685::Requests,
};
use alloy_primitives::{Address, Bytes, B128, B256, U256};
use alloy_rpc_types_engine::{
    BlobsBundleV1, BlobsBundleV2, ExecutionPayloadBodyV1 as LegacyExecutionPayloadBodyV1,
    ExecutionPayloadBodyV2 as LegacyExecutionPayloadBodyV2,
    ExecutionPayloadEnvelopeV2 as LegacyBuiltPayloadShanghai,
    ExecutionPayloadEnvelopeV4 as LegacyBuiltPayloadPrague,
    ExecutionPayloadEnvelopeV5 as LegacyBuiltPayloadOsaka,
    ExecutionPayloadEnvelopeV6 as LegacyBuiltPayloadAmsterdam, ExecutionPayloadFieldV2,
    ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3, ExecutionPayloadV4,
    ForkchoiceState, ForkchoiceUpdated as LegacyForkchoice,
    PayloadAttributes as LegacyPayloadAttributes, PayloadId, PayloadStatus as LegacyPayloadStatus,
    PayloadStatusEnum,
};

type ErrorBytes = Vec<u8>;

/// Maximum number of blobs in a REST-SSZ blob request or response.
pub const MAX_BLOBS_REQUEST: usize = 128;

/// Maximum number of payload bodies in a REST-SSZ request or response.
pub const MAX_BODIES_REQUEST: usize = 32;

/// Maximum number of entries in each execution-witness byte-list field.
pub const MAX_EXECUTION_WITNESS_ENTRIES: usize = 1_048_576;

/// Maximum byte length for a single execution-witness node, code, or header entry.
pub const MAX_EXECUTION_WITNESS_BYTES: usize = 1_048_576;

/// An Engine API v2 SSZ optional encoded as `List[T, 1]`.
///
/// This differs from [`Option<T>`]'s `ethereum_ssz` encoding, which uses an SSZ union.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Optional<T>(Vec<T>);

impl<T> Optional<T> {
    /// Creates an absent optional.
    pub const fn none() -> Self {
        Self(Vec::new())
    }

    /// Creates a present optional.
    pub fn some(value: T) -> Self {
        Self(vec![value])
    }

    /// Returns the contained value, if present.
    pub fn as_ref(&self) -> Option<&T> {
        self.0.first()
    }

    /// Returns true if no value is present.
    pub const fn is_none(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns true if a value is present.
    pub const fn is_some(&self) -> bool {
        !self.is_none()
    }

    /// Converts into a Rust optional.
    pub fn into_option(mut self) -> Option<T> {
        self.0.pop()
    }
}

impl<T> From<Option<T>> for Optional<T> {
    fn from(value: Option<T>) -> Self {
        value.map_or_else(Self::none, Self::some)
    }
}

impl<T> From<Optional<T>> for Option<T> {
    fn from(value: Optional<T>) -> Self {
        value.into_option()
    }
}

impl<T: ssz::Encode> ssz::Encode for Optional<T> {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn ssz_bytes_len(&self) -> usize {
        self.0.ssz_bytes_len()
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        self.0.ssz_append(buf);
    }
}

impl<T: ssz::Decode> ssz::Decode for Optional<T> {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        let values = Vec::<T>::from_ssz_bytes(bytes)?;
        if values.len() > 1 {
            return Err(ssz::DecodeError::BytesInvalid("optional has more than one value".into()))
        }
        Ok(Self(values))
    }
}

/// Engine API v2 REST-SSZ payload status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayloadStatus {
    /// Payload validation status.
    pub status: PayloadStatusEnum,
    /// Most recent valid block hash.
    pub latest_valid_hash: Optional<B256>,
    /// Optional payload validation error bytes.
    pub validation_error: Optional<ErrorBytes>,
}

const MAX_ERROR_BYTES: usize = 1024;

const fn status_code(status: &PayloadStatusEnum) -> u8 {
    match status {
        PayloadStatusEnum::Valid => 0,
        PayloadStatusEnum::Invalid { .. } => 1,
        PayloadStatusEnum::Syncing => 2,
        PayloadStatusEnum::Accepted => 3,
    }
}

fn status_from_code(code: u8) -> Result<PayloadStatusEnum, ssz::DecodeError> {
    match code {
        0 => Ok(PayloadStatusEnum::Valid),
        1 => Ok(PayloadStatusEnum::Invalid { validation_error: String::new() }),
        2 => Ok(PayloadStatusEnum::Syncing),
        3 => Ok(PayloadStatusEnum::Accepted),
        _ => Err(ssz::DecodeError::BytesInvalid("unknown payload status code".into())),
    }
}

fn legacy_validation_error(
    status: &PayloadStatusEnum,
) -> Result<Optional<ErrorBytes>, ConversionError> {
    match status {
        PayloadStatusEnum::Invalid { validation_error } => {
            let bytes = validation_error.as_bytes().to_vec();
            if bytes.len() > MAX_ERROR_BYTES {
                return Err(ConversionError::ErrorBytesTooLong)
            }
            Ok(Optional::some(bytes))
        }
        _ => Ok(Optional::none()),
    }
}

impl ssz::Encode for PayloadStatus {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn ssz_bytes_len(&self) -> usize {
        1 + ssz::BYTES_PER_LENGTH_OFFSET * 2 +
            self.latest_valid_hash.ssz_bytes_len() +
            self.validation_error.ssz_bytes_len()
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        let mut encoder = ssz::SszEncoder::container(buf, 1 + ssz::BYTES_PER_LENGTH_OFFSET * 2);
        encoder.append(&status_code(&self.status));
        encoder.append(&self.latest_valid_hash);
        encoder.append(&self.validation_error);
        encoder.finalize();
    }
}

impl ssz::Decode for PayloadStatus {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        let mut builder = ssz::SszDecoderBuilder::new(bytes);
        builder.register_type::<u8>()?;
        builder.register_type::<Optional<B256>>()?;
        builder.register_type::<Optional<ErrorBytes>>()?;
        let mut decoder = builder.build()?;
        let mut status = status_from_code(decoder.decode_next()?)?;
        let latest_valid_hash = decoder.decode_next()?;
        let validation_error: Optional<ErrorBytes> = decoder.decode_next()?;
        if let PayloadStatusEnum::Invalid { validation_error: error } = &mut status {
            *error = match validation_error.as_ref() {
                Some(error) => String::from_utf8(error.clone())
                    .map_err(|err| ssz::DecodeError::BytesInvalid(err.to_string()))?,
                None => String::new(),
            };
            if validation_error.as_ref().is_some_and(|error| error.len() > MAX_ERROR_BYTES) {
                return Err(ssz::DecodeError::BytesInvalid(
                    "payload validation error is too long".into(),
                ))
            }
        } else if validation_error.is_some() {
            return Err(ssz::DecodeError::BytesInvalid(
                "validation error is only valid for INVALID status".into(),
            ));
        }
        Ok(Self { status, latest_valid_hash, validation_error })
    }
}

/// Error converting legacy Engine API values into v2 REST-SSZ values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConversionError {
    /// Payload validation error exceeded the REST-SSZ byte bound.
    ErrorBytesTooLong,
    /// `ACCEPTED` is not permitted in a forkchoice response.
    AcceptedForkchoice,
    /// A bounded REST-SSZ list exceeded its maximum length.
    TooManyItems {
        /// Name of the field that exceeded its bound.
        field: &'static str,
        /// Maximum permitted item count.
        max: usize,
        /// Actual item count.
        actual: usize,
    },
}

impl core::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ErrorBytesTooLong => f.write_str("payload validation error is too long"),
            Self::AcceptedForkchoice => {
                f.write_str("ACCEPTED is not valid in a forkchoice response")
            }
            Self::TooManyItems { field, max, actual } => {
                write!(f, "too many {field}: expected at most {max}, got {actual}")
            }
        }
    }
}

impl core::error::Error for ConversionError {}

impl TryFrom<LegacyPayloadStatus> for PayloadStatus {
    type Error = ConversionError;

    fn try_from(value: LegacyPayloadStatus) -> Result<Self, Self::Error> {
        let validation_error = legacy_validation_error(&value.status)?;
        Ok(Self {
            status: value.status,
            latest_valid_hash: value.latest_valid_hash.into(),
            validation_error,
        })
    }
}

impl From<PayloadStatus> for LegacyPayloadStatus {
    fn from(value: PayloadStatus) -> Self {
        let status = match value.status {
            PayloadStatusEnum::Invalid { .. } => PayloadStatusEnum::Invalid {
                validation_error: value
                    .validation_error
                    .as_ref()
                    .and_then(|error| String::from_utf8(error.clone()).ok())
                    .unwrap_or_default(),
            },
            status => status,
        };
        Self { status, latest_valid_hash: value.latest_valid_hash.into() }
    }
}

/// Engine API v2 REST-SSZ forkchoice update response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkchoiceUpdateResponse {
    /// Restricted payload status; `ACCEPTED` is invalid here.
    pub payload_status: PayloadStatus,
    /// Opaque server-assigned payload identifier.
    pub payload_id: Optional<PayloadId>,
}

impl ssz::Encode for ForkchoiceUpdateResponse {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn ssz_bytes_len(&self) -> usize {
        ssz::BYTES_PER_LENGTH_OFFSET * 2 +
            self.payload_status.ssz_bytes_len() +
            self.payload_id.ssz_bytes_len()
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        let mut encoder = ssz::SszEncoder::container(buf, ssz::BYTES_PER_LENGTH_OFFSET * 2);
        encoder.append(&self.payload_status);
        encoder.append(&self.payload_id);
        encoder.finalize();
    }
}

impl ssz::Decode for ForkchoiceUpdateResponse {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        let mut builder = ssz::SszDecoderBuilder::new(bytes);
        builder.register_type::<PayloadStatus>()?;
        builder.register_type::<Optional<PayloadId>>()?;
        let mut decoder = builder.build()?;
        let response =
            Self { payload_status: decoder.decode_next()?, payload_id: decoder.decode_next()? };
        if matches!(response.payload_status.status, PayloadStatusEnum::Accepted) {
            return Err(ssz::DecodeError::BytesInvalid(
                "ACCEPTED is not valid in a forkchoice response".into(),
            ));
        }
        Ok(response)
    }
}

impl TryFrom<LegacyForkchoice> for ForkchoiceUpdateResponse {
    type Error = ConversionError;

    fn try_from(value: LegacyForkchoice) -> Result<Self, Self::Error> {
        let payload_status = PayloadStatus::try_from(value.payload_status)?;
        if matches!(payload_status.status, PayloadStatusEnum::Accepted) {
            return Err(ConversionError::AcceptedForkchoice)
        }
        Ok(Self { payload_status, payload_id: value.payload_id.into() })
    }
}

impl From<ForkchoiceUpdateResponse> for LegacyForkchoice {
    fn from(value: ForkchoiceUpdateResponse) -> Self {
        Self { payload_status: value.payload_status.into(), payload_id: value.payload_id.into() }
    }
}

/// Paris execution payload.
pub type ExecutionPayloadParis = ExecutionPayloadV1;

/// Shanghai execution payload.
pub type ExecutionPayloadShanghai = ExecutionPayloadV2;

/// Cancun execution payload.
pub type ExecutionPayloadCancun = ExecutionPayloadV3;

/// Prague execution payload.
pub type ExecutionPayloadPrague = ExecutionPayloadV3;

/// Osaka execution payload.
pub type ExecutionPayloadOsaka = ExecutionPayloadV3;

/// Amsterdam execution payload.
pub type ExecutionPayloadAmsterdam = ExecutionPayloadV4;

/// Paris payload attributes.
#[derive(Clone, Debug, Default, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct PayloadAttributesParis {
    /// Payload timestamp.
    pub timestamp: u64,
    /// Previous RANDAO value.
    pub prev_randao: B256,
    /// Suggested fee recipient.
    pub suggested_fee_recipient: Address,
}

/// Shanghai payload attributes.
#[derive(Clone, Debug, Default, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct PayloadAttributesShanghai {
    /// Payload timestamp.
    pub timestamp: u64,
    /// Previous RANDAO value.
    pub prev_randao: B256,
    /// Suggested fee recipient.
    pub suggested_fee_recipient: Address,
    /// Withdrawals to include in the payload.
    pub withdrawals: Vec<Withdrawal>,
}

/// Cancun payload attributes.
#[derive(Clone, Debug, Default, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct PayloadAttributesCancun {
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
}

/// Prague uses the Cancun payload-attributes schema.
pub type PayloadAttributesPrague = PayloadAttributesCancun;

/// Osaka uses the Cancun payload-attributes schema.
pub type PayloadAttributesOsaka = PayloadAttributesCancun;

/// Amsterdam payload attributes.
#[derive(Clone, Debug, Default, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct PayloadAttributesAmsterdam {
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
}

/// Error converting legacy cross-fork payload attributes into a fork-specific SSZ container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadAttributesConversionError {
    /// A field required by the selected fork is absent.
    MissingField(&'static str),
    /// A field from a later fork is populated and would be lost.
    UnexpectedField(&'static str),
}

impl core::fmt::Display for PayloadAttributesConversionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MissingField(field) => {
                write!(f, "missing required payload attributes field: {field}")
            }
            Self::UnexpectedField(field) => {
                write!(f, "unexpected later-fork payload attributes field: {field}")
            }
        }
    }
}

impl core::error::Error for PayloadAttributesConversionError {}

const fn ensure_absent<T>(
    value: &Option<T>,
    field: &'static str,
) -> Result<(), PayloadAttributesConversionError> {
    if value.is_some() {
        Err(PayloadAttributesConversionError::UnexpectedField(field))
    } else {
        Ok(())
    }
}

fn require<T>(
    value: Option<T>,
    field: &'static str,
) -> Result<T, PayloadAttributesConversionError> {
    value.ok_or(PayloadAttributesConversionError::MissingField(field))
}

impl From<PayloadAttributesParis> for LegacyPayloadAttributes {
    fn from(value: PayloadAttributesParis) -> Self {
        Self {
            timestamp: value.timestamp,
            prev_randao: value.prev_randao,
            suggested_fee_recipient: value.suggested_fee_recipient,
            withdrawals: None,
            parent_beacon_block_root: None,
            slot_number: None,
            target_gas_limit: None,
        }
    }
}

impl TryFrom<LegacyPayloadAttributes> for PayloadAttributesParis {
    type Error = PayloadAttributesConversionError;

    fn try_from(value: LegacyPayloadAttributes) -> Result<Self, Self::Error> {
        ensure_absent(&value.withdrawals, "withdrawals")?;
        ensure_absent(&value.parent_beacon_block_root, "parent_beacon_block_root")?;
        ensure_absent(&value.slot_number, "slot_number")?;
        ensure_absent(&value.target_gas_limit, "target_gas_limit")?;
        Ok(Self {
            timestamp: value.timestamp,
            prev_randao: value.prev_randao,
            suggested_fee_recipient: value.suggested_fee_recipient,
        })
    }
}

impl From<PayloadAttributesShanghai> for LegacyPayloadAttributes {
    fn from(value: PayloadAttributesShanghai) -> Self {
        Self {
            timestamp: value.timestamp,
            prev_randao: value.prev_randao,
            suggested_fee_recipient: value.suggested_fee_recipient,
            withdrawals: Some(value.withdrawals),
            parent_beacon_block_root: None,
            slot_number: None,
            target_gas_limit: None,
        }
    }
}

impl TryFrom<LegacyPayloadAttributes> for PayloadAttributesShanghai {
    type Error = PayloadAttributesConversionError;

    fn try_from(value: LegacyPayloadAttributes) -> Result<Self, Self::Error> {
        ensure_absent(&value.parent_beacon_block_root, "parent_beacon_block_root")?;
        ensure_absent(&value.slot_number, "slot_number")?;
        ensure_absent(&value.target_gas_limit, "target_gas_limit")?;
        Ok(Self {
            timestamp: value.timestamp,
            prev_randao: value.prev_randao,
            suggested_fee_recipient: value.suggested_fee_recipient,
            withdrawals: require(value.withdrawals, "withdrawals")?,
        })
    }
}

impl From<PayloadAttributesCancun> for LegacyPayloadAttributes {
    fn from(value: PayloadAttributesCancun) -> Self {
        Self {
            timestamp: value.timestamp,
            prev_randao: value.prev_randao,
            suggested_fee_recipient: value.suggested_fee_recipient,
            withdrawals: Some(value.withdrawals),
            parent_beacon_block_root: Some(value.parent_beacon_block_root),
            slot_number: None,
            target_gas_limit: None,
        }
    }
}

impl TryFrom<LegacyPayloadAttributes> for PayloadAttributesCancun {
    type Error = PayloadAttributesConversionError;

    fn try_from(value: LegacyPayloadAttributes) -> Result<Self, Self::Error> {
        ensure_absent(&value.slot_number, "slot_number")?;
        ensure_absent(&value.target_gas_limit, "target_gas_limit")?;
        Ok(Self {
            timestamp: value.timestamp,
            prev_randao: value.prev_randao,
            suggested_fee_recipient: value.suggested_fee_recipient,
            withdrawals: require(value.withdrawals, "withdrawals")?,
            parent_beacon_block_root: require(
                value.parent_beacon_block_root,
                "parent_beacon_block_root",
            )?,
        })
    }
}

impl From<PayloadAttributesAmsterdam> for LegacyPayloadAttributes {
    fn from(value: PayloadAttributesAmsterdam) -> Self {
        Self {
            timestamp: value.timestamp,
            prev_randao: value.prev_randao,
            suggested_fee_recipient: value.suggested_fee_recipient,
            withdrawals: Some(value.withdrawals),
            parent_beacon_block_root: Some(value.parent_beacon_block_root),
            slot_number: Some(value.slot_number),
            target_gas_limit: Some(value.target_gas_limit),
        }
    }
}

impl TryFrom<LegacyPayloadAttributes> for PayloadAttributesAmsterdam {
    type Error = PayloadAttributesConversionError;

    fn try_from(value: LegacyPayloadAttributes) -> Result<Self, Self::Error> {
        Ok(Self {
            timestamp: value.timestamp,
            prev_randao: value.prev_randao,
            suggested_fee_recipient: value.suggested_fee_recipient,
            withdrawals: require(value.withdrawals, "withdrawals")?,
            parent_beacon_block_root: require(
                value.parent_beacon_block_root,
                "parent_beacon_block_root",
            )?,
            slot_number: require(value.slot_number, "slot_number")?,
            target_gas_limit: require(value.target_gas_limit, "target_gas_limit")?,
        })
    }
}

/// This structure maps to the Engine API v2 REST-SSZ payload-build response for Paris.
///
/// Unlike the legacy `engine_getPayloadV1` response, this includes the expected block value.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct BuiltPayloadParis {
    /// Execution payload V1.
    pub payload: ExecutionPayloadParis,
    /// The expected value to be received by the fee recipient in wei.
    pub block_value: U256,
}

/// This structure maps to the Engine API v2 REST-SSZ payload-build response for Shanghai.
///
/// This follows the legacy `engine_getPayloadV2` payload-build response shape: execution payload
/// plus block value only. `should_override_builder` starts at Cancun.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct BuiltPayloadShanghai {
    /// Execution payload V2.
    pub payload: ExecutionPayloadShanghai,
    /// The expected value to be received by the fee recipient in wei.
    pub block_value: U256,
}

/// Engine API v2 REST-SSZ payload-build response for Cancun.
///
/// This is wire-compatible with the legacy `engine_getPayloadV3` response envelope.
pub type BuiltPayloadCancun = alloy_rpc_types_engine::ExecutionPayloadEnvelopeV3;

/// This structure maps to the Engine API v2 REST-SSZ payload-build response for Prague.
///
/// Unlike the legacy [`alloy_rpc_types_engine::ExecutionPayloadEnvelopeV4`],
/// `execution_requests` precedes `should_override_builder` in the normative SSZ field order.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct BuiltPayloadPrague {
    /// Execution payload V3.
    pub payload: ExecutionPayloadPrague,
    /// The expected value to be received by the fee recipient in wei.
    pub block_value: U256,
    /// The blobs, commitments, and proofs associated with the executed payload.
    pub blobs_bundle: BlobsBundleV1,
    /// A list of opaque EIP-7685 requests.
    pub execution_requests: Requests,
    /// A suggestion from the execution layer whether this payload should be used instead of an
    /// externally provided one.
    pub should_override_builder: bool,
}

/// This structure maps to the Engine API v2 REST-SSZ payload-build response for Osaka.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct BuiltPayloadOsaka {
    /// Execution payload V3.
    pub payload: ExecutionPayloadOsaka,
    /// The expected value to be received by the fee recipient in wei.
    pub block_value: U256,
    /// The blobs, commitments, and EIP-7594 cell proofs associated with the executed payload.
    pub blobs_bundle: BlobsBundleV2,
    /// A list of opaque EIP-7685 requests.
    pub execution_requests: Requests,
    /// A suggestion from the execution layer whether this payload should be used instead of an
    /// externally provided one.
    pub should_override_builder: bool,
}

/// This structure maps to the Engine API v2 REST-SSZ payload-build response for Amsterdam.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct BuiltPayloadAmsterdam {
    /// Execution payload V4.
    pub payload: ExecutionPayloadAmsterdam,
    /// The expected value to be received by the fee recipient in wei.
    pub block_value: U256,
    /// The blobs, commitments, and EIP-7594 cell proofs associated with the executed payload.
    pub blobs_bundle: BlobsBundleV2,
    /// A list of opaque EIP-7685 requests.
    pub execution_requests: Requests,
    /// A suggestion from the execution layer whether this payload should be used instead of an
    /// externally provided one.
    pub should_override_builder: bool,
}

/// Error converting legacy payload-build envelopes into fork-specific REST-SSZ containers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltPayloadConversionError {
    /// The legacy envelope carried an execution payload from a different fork.
    UnexpectedPayloadFork(&'static str),
}

impl core::fmt::Display for BuiltPayloadConversionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnexpectedPayloadFork(fork) => {
                write!(f, "unexpected execution payload fork: {fork}")
            }
        }
    }
}

impl core::error::Error for BuiltPayloadConversionError {}

impl From<BuiltPayloadShanghai> for LegacyBuiltPayloadShanghai {
    fn from(value: BuiltPayloadShanghai) -> Self {
        Self {
            execution_payload: ExecutionPayloadFieldV2::V2(value.payload),
            block_value: value.block_value,
        }
    }
}

impl TryFrom<LegacyBuiltPayloadShanghai> for BuiltPayloadShanghai {
    type Error = BuiltPayloadConversionError;

    fn try_from(value: LegacyBuiltPayloadShanghai) -> Result<Self, Self::Error> {
        match value.execution_payload {
            ExecutionPayloadFieldV2::V2(payload) => {
                Ok(Self { payload, block_value: value.block_value })
            }
            ExecutionPayloadFieldV2::V1(_) => {
                Err(BuiltPayloadConversionError::UnexpectedPayloadFork("Paris"))
            }
        }
    }
}

impl From<LegacyBuiltPayloadPrague> for BuiltPayloadPrague {
    fn from(value: LegacyBuiltPayloadPrague) -> Self {
        Self {
            payload: value.envelope_inner.execution_payload,
            block_value: value.envelope_inner.block_value,
            blobs_bundle: value.envelope_inner.blobs_bundle,
            execution_requests: value.execution_requests,
            should_override_builder: value.envelope_inner.should_override_builder,
        }
    }
}

impl From<BuiltPayloadPrague> for LegacyBuiltPayloadPrague {
    fn from(value: BuiltPayloadPrague) -> Self {
        Self {
            envelope_inner: alloy_rpc_types_engine::ExecutionPayloadEnvelopeV3 {
                execution_payload: value.payload,
                block_value: value.block_value,
                blobs_bundle: value.blobs_bundle,
                should_override_builder: value.should_override_builder,
            },
            execution_requests: value.execution_requests,
        }
    }
}

impl From<LegacyBuiltPayloadOsaka> for BuiltPayloadOsaka {
    fn from(value: LegacyBuiltPayloadOsaka) -> Self {
        Self {
            payload: value.execution_payload,
            block_value: value.block_value,
            blobs_bundle: value.blobs_bundle,
            execution_requests: value.execution_requests,
            should_override_builder: value.should_override_builder,
        }
    }
}

impl From<BuiltPayloadOsaka> for LegacyBuiltPayloadOsaka {
    fn from(value: BuiltPayloadOsaka) -> Self {
        Self {
            execution_payload: value.payload,
            block_value: value.block_value,
            blobs_bundle: value.blobs_bundle,
            should_override_builder: value.should_override_builder,
            execution_requests: value.execution_requests,
        }
    }
}

impl From<LegacyBuiltPayloadAmsterdam> for BuiltPayloadAmsterdam {
    fn from(value: LegacyBuiltPayloadAmsterdam) -> Self {
        Self {
            payload: value.execution_payload,
            block_value: value.block_value,
            blobs_bundle: value.blobs_bundle,
            execution_requests: value.execution_requests,
            should_override_builder: value.should_override_builder,
        }
    }
}

impl From<BuiltPayloadAmsterdam> for LegacyBuiltPayloadAmsterdam {
    fn from(value: BuiltPayloadAmsterdam) -> Self {
        Self {
            execution_payload: value.payload,
            block_value: value.block_value,
            blobs_bundle: value.blobs_bundle,
            should_override_builder: value.should_override_builder,
            execution_requests: value.execution_requests,
        }
    }
}

/// REST-SSZ payload-submission request containers.
///
/// These are distinct from the legacy Engine JSON-RPC get-payload envelopes: submission requests
/// are fork-specific request bodies, while the legacy envelope types mostly model get-payload
/// responses and sometimes carry response-only fields such as block value, blob bundles, builder
/// override hints, or a different field order.
///
/// Paris payload-submission request.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ExecutionPayloadEnvelopeParis {
    /// Submitted execution payload.
    pub payload: ExecutionPayloadParis,
}

/// Shanghai payload-submission request.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ExecutionPayloadEnvelopeShanghai {
    /// Submitted execution payload.
    pub payload: ExecutionPayloadShanghai,
}

/// Cancun payload-submission request.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ExecutionPayloadEnvelopeCancun {
    /// Submitted execution payload.
    pub payload: ExecutionPayloadCancun,
    /// Root of the parent beacon block.
    pub parent_beacon_block_root: B256,
}

/// Prague payload-submission request.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ExecutionPayloadEnvelopePrague {
    /// Submitted execution payload.
    pub payload: ExecutionPayloadPrague,
    /// Root of the parent beacon block.
    pub parent_beacon_block_root: B256,
    /// EIP-7685 execution requests.
    pub execution_requests: Requests,
}

/// Osaka payload-submission request.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ExecutionPayloadEnvelopeOsaka {
    /// Submitted execution payload.
    pub payload: ExecutionPayloadOsaka,
    /// Root of the parent beacon block.
    pub parent_beacon_block_root: B256,
    /// EIP-7685 execution requests.
    pub execution_requests: Requests,
}

/// Amsterdam payload-submission request.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ExecutionPayloadEnvelopeAmsterdam {
    /// Submitted execution payload.
    pub payload: ExecutionPayloadAmsterdam,
    /// Root of the parent beacon block.
    pub parent_beacon_block_root: B256,
    /// EIP-7685 execution requests.
    pub execution_requests: Requests,
}

impl From<ExecutionPayloadParis> for ExecutionPayloadEnvelopeParis {
    fn from(payload: ExecutionPayloadParis) -> Self {
        Self { payload }
    }
}

impl From<ExecutionPayloadShanghai> for ExecutionPayloadEnvelopeShanghai {
    fn from(payload: ExecutionPayloadShanghai) -> Self {
        Self { payload }
    }
}

impl From<(ExecutionPayloadCancun, B256)> for ExecutionPayloadEnvelopeCancun {
    fn from((payload, parent_beacon_block_root): (ExecutionPayloadCancun, B256)) -> Self {
        Self { payload, parent_beacon_block_root }
    }
}

impl From<(ExecutionPayloadPrague, B256, Requests)> for ExecutionPayloadEnvelopePrague {
    fn from(
        (payload, parent_beacon_block_root, execution_requests): (
            ExecutionPayloadPrague,
            B256,
            Requests,
        ),
    ) -> Self {
        Self { payload, parent_beacon_block_root, execution_requests }
    }
}

impl From<(ExecutionPayloadOsaka, B256, Requests)> for ExecutionPayloadEnvelopeOsaka {
    fn from(
        (payload, parent_beacon_block_root, execution_requests): (
            ExecutionPayloadOsaka,
            B256,
            Requests,
        ),
    ) -> Self {
        Self { payload, parent_beacon_block_root, execution_requests }
    }
}

impl From<(ExecutionPayloadAmsterdam, B256, Requests)> for ExecutionPayloadEnvelopeAmsterdam {
    fn from(
        (payload, parent_beacon_block_root, execution_requests): (
            ExecutionPayloadAmsterdam,
            B256,
            Requests,
        ),
    ) -> Self {
        Self { payload, parent_beacon_block_root, execution_requests }
    }
}

/// Paris forkchoice-update request.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ForkchoiceUpdateParis {
    /// Current forkchoice state.
    pub forkchoice_state: ForkchoiceState,
    /// Optional Paris payload attributes.
    pub payload_attributes: Optional<PayloadAttributesParis>,
}

/// Shanghai forkchoice-update request.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ForkchoiceUpdateShanghai {
    /// Current forkchoice state.
    pub forkchoice_state: ForkchoiceState,
    /// Optional Shanghai payload attributes.
    pub payload_attributes: Optional<PayloadAttributesShanghai>,
}

/// Cancun forkchoice-update request.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ForkchoiceUpdateCancun {
    /// Current forkchoice state.
    pub forkchoice_state: ForkchoiceState,
    /// Optional Cancun payload attributes.
    pub payload_attributes: Optional<PayloadAttributesCancun>,
}

/// Prague forkchoice-update request.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ForkchoiceUpdatePrague {
    /// Current forkchoice state.
    pub forkchoice_state: ForkchoiceState,
    /// Optional Prague payload attributes.
    pub payload_attributes: Optional<PayloadAttributesPrague>,
}

/// Osaka forkchoice-update request.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ForkchoiceUpdateOsaka {
    /// Current forkchoice state.
    pub forkchoice_state: ForkchoiceState,
    /// Optional Osaka payload attributes.
    pub payload_attributes: Optional<PayloadAttributesOsaka>,
}

/// Amsterdam forkchoice-update request.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ForkchoiceUpdateAmsterdam {
    /// Current forkchoice state.
    pub forkchoice_state: ForkchoiceState,
    /// Optional Amsterdam payload attributes.
    pub payload_attributes: Optional<PayloadAttributesAmsterdam>,
    /// Optional `Bitvector[128]` custody-column selection.
    pub custody_columns: Optional<B128>,
}

/// Fork-specific execution payload body for Paris.
#[derive(Clone, Debug, Default, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ExecutionPayloadBodyParis {
    /// Enveloped encoded transactions.
    pub transactions: Vec<Bytes>,
}

/// Fork-specific execution payload body for Shanghai.
#[derive(Clone, Debug, Default, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ExecutionPayloadBodyShanghai {
    /// Enveloped encoded transactions.
    pub transactions: Vec<Bytes>,
    /// Withdrawals included in the block.
    pub withdrawals: Vec<Withdrawal>,
}

/// Cancun uses the Shanghai execution-payload-body schema.
pub type ExecutionPayloadBodyCancun = ExecutionPayloadBodyShanghai;

/// Prague uses the Shanghai execution-payload-body schema.
pub type ExecutionPayloadBodyPrague = ExecutionPayloadBodyShanghai;

/// Osaka uses the Shanghai execution-payload-body schema.
pub type ExecutionPayloadBodyOsaka = ExecutionPayloadBodyShanghai;

/// Fork-specific execution payload body for Amsterdam.
#[derive(Clone, Debug, Default, PartialEq, Eq, ssz_derive::Encode, ssz_derive::Decode)]
pub struct ExecutionPayloadBodyAmsterdam {
    /// Enveloped encoded transactions.
    pub transactions: Vec<Bytes>,
    /// Withdrawals included in the block.
    pub withdrawals: Vec<Withdrawal>,
    /// RLP-encoded EIP-7928 block access list.
    pub block_access_list: Bytes,
}

/// Error converting legacy cross-fork execution payload bodies into fork-specific containers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionPayloadBodyConversionError {
    /// A field required by the selected fork is absent.
    MissingField(&'static str),
    /// A field from a later fork is populated and would be lost.
    UnexpectedField(&'static str),
}

impl core::fmt::Display for ExecutionPayloadBodyConversionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MissingField(field) => {
                write!(f, "missing required execution payload body field: {field}")
            }
            Self::UnexpectedField(field) => {
                write!(f, "unexpected later-fork execution payload body field: {field}")
            }
        }
    }
}

impl core::error::Error for ExecutionPayloadBodyConversionError {}

impl From<ExecutionPayloadBodyParis> for LegacyExecutionPayloadBodyV1 {
    fn from(value: ExecutionPayloadBodyParis) -> Self {
        Self { transactions: value.transactions, withdrawals: None }
    }
}

impl TryFrom<LegacyExecutionPayloadBodyV1> for ExecutionPayloadBodyParis {
    type Error = ExecutionPayloadBodyConversionError;

    fn try_from(value: LegacyExecutionPayloadBodyV1) -> Result<Self, Self::Error> {
        if value.withdrawals.is_some() {
            return Err(ExecutionPayloadBodyConversionError::UnexpectedField("withdrawals"))
        }
        Ok(Self { transactions: value.transactions })
    }
}

impl From<ExecutionPayloadBodyShanghai> for LegacyExecutionPayloadBodyV1 {
    fn from(value: ExecutionPayloadBodyShanghai) -> Self {
        Self { transactions: value.transactions, withdrawals: Some(value.withdrawals) }
    }
}

impl TryFrom<LegacyExecutionPayloadBodyV1> for ExecutionPayloadBodyShanghai {
    type Error = ExecutionPayloadBodyConversionError;

    fn try_from(value: LegacyExecutionPayloadBodyV1) -> Result<Self, Self::Error> {
        Ok(Self {
            transactions: value.transactions,
            withdrawals: value
                .withdrawals
                .ok_or(ExecutionPayloadBodyConversionError::MissingField("withdrawals"))?,
        })
    }
}

impl From<ExecutionPayloadBodyAmsterdam> for LegacyExecutionPayloadBodyV2 {
    fn from(value: ExecutionPayloadBodyAmsterdam) -> Self {
        Self {
            transactions: value.transactions,
            withdrawals: Some(value.withdrawals),
            block_access_list: Some(value.block_access_list),
        }
    }
}

impl TryFrom<LegacyExecutionPayloadBodyV2> for ExecutionPayloadBodyAmsterdam {
    type Error = ExecutionPayloadBodyConversionError;

    fn try_from(value: LegacyExecutionPayloadBodyV2) -> Result<Self, Self::Error> {
        Ok(Self {
            transactions: value.transactions,
            withdrawals: value
                .withdrawals
                .ok_or(ExecutionPayloadBodyConversionError::MissingField("withdrawals"))?,
            block_access_list: value
                .block_access_list
                .ok_or(ExecutionPayloadBodyConversionError::MissingField("block_access_list"))?,
        })
    }
}

/// REST-SSZ historical bodies-by-hash request.
///
/// This is a single-field container, not a bare SSZ list.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode)]
pub struct BodiesByHashRequest {
    /// Requested block hashes.
    pub block_hashes: Vec<B256>,
}

impl ssz::Decode for BodiesByHashRequest {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        let mut builder = ssz::SszDecoderBuilder::new(bytes);
        builder.register_type::<Vec<B256>>()?;
        let mut decoder = builder.build()?;
        let request = Self { block_hashes: decoder.decode_next()? };
        if request.block_hashes.len() > MAX_BODIES_REQUEST {
            return Err(ssz::DecodeError::BytesInvalid(format!(
                "too many payload body hashes: expected at most {MAX_BODIES_REQUEST}, got {}",
                request.block_hashes.len()
            )))
        }
        Ok(request)
    }
}

/// Historical body response entry with explicit availability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyEntry<T> {
    /// Whether the body is available and belongs to the requested fork.
    pub available: bool,
    /// Fork-specific body, ignored when `available` is false.
    pub body: T,
}

impl<T> BodyEntry<T> {
    /// Creates an available body entry.
    pub const fn available(body: T) -> Self {
        Self { available: true, body }
    }
}

impl<T: Default> BodyEntry<T> {
    /// Creates an unavailable body entry.
    pub fn unavailable() -> Self {
        Self { available: false, body: T::default() }
    }
}

impl<T: ssz::Encode> ssz::Encode for BodyEntry<T> {
    fn is_ssz_fixed_len() -> bool {
        T::is_ssz_fixed_len()
    }

    fn ssz_fixed_len() -> usize {
        if T::is_ssz_fixed_len() {
            1 + T::ssz_fixed_len()
        } else {
            1 + ssz::BYTES_PER_LENGTH_OFFSET
        }
    }

    fn ssz_bytes_len(&self) -> usize {
        1 + if T::is_ssz_fixed_len() {
            self.body.ssz_bytes_len()
        } else {
            ssz::BYTES_PER_LENGTH_OFFSET + self.body.ssz_bytes_len()
        }
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        let fixed_len = 1 + if T::is_ssz_fixed_len() {
            T::ssz_fixed_len()
        } else {
            ssz::BYTES_PER_LENGTH_OFFSET
        };
        let mut encoder = ssz::SszEncoder::container(buf, fixed_len);
        encoder.append(&self.available);
        encoder.append(&self.body);
        encoder.finalize();
    }
}

impl<T: ssz::Decode> ssz::Decode for BodyEntry<T> {
    fn is_ssz_fixed_len() -> bool {
        T::is_ssz_fixed_len()
    }

    fn ssz_fixed_len() -> usize {
        if T::is_ssz_fixed_len() {
            1 + T::ssz_fixed_len()
        } else {
            1 + ssz::BYTES_PER_LENGTH_OFFSET
        }
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        let mut builder = ssz::SszDecoderBuilder::new(bytes);
        builder.register_type::<bool>()?;
        builder.register_type::<T>()?;
        let mut decoder = builder.build()?;
        Ok(Self { available: decoder.decode_next()?, body: decoder.decode_next()? })
    }
}

/// Bounded REST-SSZ historical bodies response.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode)]
pub struct BodiesResponse<T> {
    /// Body entries in request or range order.
    pub entries: Vec<BodyEntry<T>>,
}

/// Error constructing a bounded REST-SSZ historical bodies response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodiesResponseConversionError {
    /// The response contains more entries than the SSZ response limit.
    TooManyEntries,
}

impl core::fmt::Display for BodiesResponseConversionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooManyEntries => f.write_str("too many payload body entries"),
        }
    }
}

impl core::error::Error for BodiesResponseConversionError {}

impl<T: Default> BodiesResponse<T> {
    /// Creates a response from optional legacy bodies.
    ///
    /// Missing bodies, or bodies that do not convert to the requested fork container, are encoded
    /// as unavailable entries.
    pub fn from_optional_bodies<LegacyBody>(
        bodies: Vec<Option<LegacyBody>>,
        convert: impl Fn(LegacyBody) -> Option<T>,
    ) -> Result<Self, BodiesResponseConversionError> {
        if bodies.len() > MAX_BODIES_REQUEST {
            return Err(BodiesResponseConversionError::TooManyEntries)
        }

        let entries = bodies
            .into_iter()
            .map(|body| match body.and_then(&convert) {
                Some(body) => BodyEntry::available(body),
                None => BodyEntry::unavailable(),
            })
            .collect();

        Ok(Self { entries })
    }
}

impl<T: ssz::Decode + 'static> ssz::Decode for BodiesResponse<T> {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        let mut builder = ssz::SszDecoderBuilder::new(bytes);
        builder.register_type::<Vec<BodyEntry<T>>>()?;
        let mut decoder = builder.build()?;
        let response = Self { entries: decoder.decode_next()? };
        if response.entries.len() > MAX_BODIES_REQUEST {
            return Err(ssz::DecodeError::BytesInvalid(format!(
                "too many payload body entries: expected at most {MAX_BODIES_REQUEST}, got {}",
                response.entries.len()
            )))
        }
        Ok(response)
    }
}

/// Paris historical bodies response.
pub type BodiesResponseParis = BodiesResponse<ExecutionPayloadBodyParis>;

/// Shanghai historical bodies response.
pub type BodiesResponseShanghai = BodiesResponse<ExecutionPayloadBodyShanghai>;

/// Cancun historical bodies response.
pub type BodiesResponseCancun = BodiesResponse<ExecutionPayloadBodyCancun>;

/// Prague historical bodies response.
pub type BodiesResponsePrague = BodiesResponse<ExecutionPayloadBodyPrague>;

/// Osaka historical bodies response.
pub type BodiesResponseOsaka = BodiesResponse<ExecutionPayloadBodyOsaka>;

/// Amsterdam historical bodies response.
pub type BodiesResponseAmsterdam = BodiesResponse<ExecutionPayloadBodyAmsterdam>;

/// V1-V3 blob request container.
///
/// This single-field container starts with a four-byte SSZ offset and is not wire-equivalent to a
/// top-level list.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode)]
pub struct BlobsV1Request {
    /// Requested versioned blob hashes.
    pub versioned_hashes: Vec<B256>,
}

/// V2 uses the V1 request schema.
pub type BlobsV2Request = BlobsV1Request;

/// V3 uses the V1 request schema.
pub type BlobsV3Request = BlobsV1Request;

impl ssz::Decode for BlobsV1Request {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        let mut builder = ssz::SszDecoderBuilder::new(bytes);
        builder.register_type::<Vec<B256>>()?;
        let mut decoder = builder.build()?;
        let request = Self { versioned_hashes: decoder.decode_next()? };
        if request.versioned_hashes.len() > MAX_BLOBS_REQUEST {
            return Err(ssz::DecodeError::BytesInvalid(format!(
                "too many blob hashes: expected at most {MAX_BLOBS_REQUEST}, got {}",
                request.versioned_hashes.len()
            )))
        }
        Ok(request)
    }
}

/// V4 blob request container with a packed 128-bit index bitvector.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode)]
pub struct BlobsV4Request {
    /// Requested versioned blob hashes.
    pub versioned_hashes: Vec<B256>,
    /// Requested cell indices, SSZ `Bitvector[128]`.
    pub indices_bitarray: B128,
}

impl ssz::Decode for BlobsV4Request {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        let mut builder = ssz::SszDecoderBuilder::new(bytes);
        builder.register_type::<Vec<B256>>()?;
        builder.register_type::<B128>()?;
        let mut decoder = builder.build()?;
        let request = Self {
            versioned_hashes: decoder.decode_next()?,
            indices_bitarray: decoder.decode_next()?,
        };
        if request.versioned_hashes.len() > MAX_BLOBS_REQUEST {
            return Err(ssz::DecodeError::BytesInvalid(format!(
                "too many blob hashes: expected at most {MAX_BLOBS_REQUEST}, got {}",
                request.versioned_hashes.len()
            )))
        }
        Ok(request)
    }
}

/// Blob response entry with explicit outer availability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobEntry<T> {
    /// Whether the complete blob contents are available.
    pub available: bool,
    /// Complete contents, or valid zero-valued contents when unavailable.
    pub contents: T,
}

impl<T: ssz::Encode> ssz::Encode for BlobEntry<T> {
    fn is_ssz_fixed_len() -> bool {
        T::is_ssz_fixed_len()
    }

    fn ssz_fixed_len() -> usize {
        if T::is_ssz_fixed_len() {
            1 + T::ssz_fixed_len()
        } else {
            1 + ssz::BYTES_PER_LENGTH_OFFSET
        }
    }

    fn ssz_bytes_len(&self) -> usize {
        1 + if T::is_ssz_fixed_len() {
            self.contents.ssz_bytes_len()
        } else {
            ssz::BYTES_PER_LENGTH_OFFSET + self.contents.ssz_bytes_len()
        }
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        let fixed_len = 1 + if T::is_ssz_fixed_len() {
            T::ssz_fixed_len()
        } else {
            ssz::BYTES_PER_LENGTH_OFFSET
        };
        let mut encoder = ssz::SszEncoder::container(buf, fixed_len);
        encoder.append(&self.available);
        encoder.append(&self.contents);
        encoder.finalize();
    }
}

impl<T: ssz::Decode> ssz::Decode for BlobEntry<T> {
    fn is_ssz_fixed_len() -> bool {
        T::is_ssz_fixed_len()
    }

    fn ssz_fixed_len() -> usize {
        if T::is_ssz_fixed_len() {
            1 + T::ssz_fixed_len()
        } else {
            1 + ssz::BYTES_PER_LENGTH_OFFSET
        }
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        let mut builder = ssz::SszDecoderBuilder::new(bytes);
        builder.register_type::<bool>()?;
        builder.register_type::<T>()?;
        let mut decoder = builder.build()?;
        Ok(Self { available: decoder.decode_next()?, contents: decoder.decode_next()? })
    }
}

/// Bounded blob response container.
#[derive(Clone, Debug, PartialEq, Eq, ssz_derive::Encode)]
pub struct BlobsResponse<T> {
    /// One response entry per requested hash.
    pub entries: Vec<BlobEntry<T>>,
}

/// V1 whole-blob response.
pub type BlobsV1Response = BlobsResponse<BlobAndProofV1>;

/// V2 all-or-nothing cell-proof response.
pub type BlobsV2Response = BlobsResponse<BlobAndProofV2>;

/// V3 partial cell-proof response.
pub type BlobsV3Response = BlobsResponse<BlobAndProofV2>;

/// V4 partial cell-range response.
pub type BlobsV4Response = BlobsResponse<BlobCellsAndProofs>;

/// Blob cells and proofs with REST-SSZ optional cell positions.
///
/// This uses [`Optional`] (`List[T, 1]`) for per-cell nullability, not Rust [`Option`]'s SSZ
/// union encoding.
#[derive(Clone, Debug, Default, PartialEq, Eq, ssz_derive::Encode)]
pub struct BlobCellsAndProofs {
    /// Requested blob cells.
    pub blob_cells: Vec<Optional<Cell>>,
    /// KZG proofs for the requested blob cells.
    pub proofs: Vec<Optional<Bytes48>>,
}

impl ssz::Decode for BlobCellsAndProofs {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        #[derive(ssz_derive::Decode)]
        struct Raw {
            blob_cells: Vec<Optional<Cell>>,
            proofs: Vec<Optional<Bytes48>>,
        }

        let raw = Raw::from_ssz_bytes(bytes)?;

        if raw.blob_cells.len() > CELLS_PER_EXT_BLOB {
            return Err(ssz::DecodeError::BytesInvalid(format!(
                "Invalid BlobCellsAndProofs: expected at most {CELLS_PER_EXT_BLOB} blob cells, got {}",
                raw.blob_cells.len()
            )))
        }

        if raw.blob_cells.len() != raw.proofs.len() {
            return Err(ssz::DecodeError::BytesInvalid(format!(
                "Invalid BlobCellsAndProofs: blob_cells length {} does not match proofs length {}",
                raw.blob_cells.len(),
                raw.proofs.len()
            )))
        }

        if raw
            .blob_cells
            .iter()
            .zip(raw.proofs.iter())
            .any(|(cell, proof)| cell.is_some() != proof.is_some())
        {
            return Err(ssz::DecodeError::BytesInvalid(
                "Invalid BlobCellsAndProofs: blob_cells and proofs must have matching optional positions"
                    .into(),
            ))
        }

        Ok(Self { blob_cells: raw.blob_cells, proofs: raw.proofs })
    }
}

impl<T: ssz::Decode + 'static> ssz::Decode for BlobsResponse<T> {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        let mut builder = ssz::SszDecoderBuilder::new(bytes);
        builder.register_type::<Vec<BlobEntry<T>>>()?;
        let mut decoder = builder.build()?;
        let response = Self { entries: decoder.decode_next()? };
        if response.entries.len() > MAX_BLOBS_REQUEST {
            return Err(ssz::DecodeError::BytesInvalid(format!(
                "too many blob response entries: expected at most {MAX_BLOBS_REQUEST}, got {}",
                response.entries.len()
            )))
        }
        Ok(response)
    }
}

fn zero_blob_v1() -> BlobAndProofV1 {
    BlobAndProofV1 { blob: Box::new(Blob::ZERO), proof: Bytes48::ZERO }
}

fn zero_blob_v2() -> BlobAndProofV2 {
    BlobAndProofV2 { blob: Box::new(Blob::ZERO), proofs: Vec::new() }
}

impl TryFrom<Vec<Option<BlobAndProofV1>>> for BlobsV1Response {
    type Error = ConversionError;

    fn try_from(value: Vec<Option<BlobAndProofV1>>) -> Result<Self, Self::Error> {
        if value.len() > MAX_BLOBS_REQUEST {
            return Err(ConversionError::TooManyItems {
                field: "blobs",
                max: MAX_BLOBS_REQUEST,
                actual: value.len(),
            })
        }

        let entries = value
            .into_iter()
            .map(|value| match value {
                Some(contents) => BlobEntry { available: true, contents },
                None => BlobEntry { available: false, contents: zero_blob_v1() },
            })
            .collect();
        Ok(Self { entries })
    }
}

impl TryFrom<Vec<BlobAndProofV2>> for BlobsV2Response {
    type Error = ConversionError;

    fn try_from(value: Vec<BlobAndProofV2>) -> Result<Self, Self::Error> {
        if value.len() > MAX_BLOBS_REQUEST {
            return Err(ConversionError::TooManyItems {
                field: "blobs",
                max: MAX_BLOBS_REQUEST,
                actual: value.len(),
            })
        }

        let entries =
            value.into_iter().map(|contents| BlobEntry { available: true, contents }).collect();
        Ok(Self { entries })
    }
}

impl TryFrom<Vec<Option<BlobAndProofV2>>> for BlobsV3Response {
    type Error = ConversionError;

    fn try_from(value: Vec<Option<BlobAndProofV2>>) -> Result<Self, Self::Error> {
        if value.len() > MAX_BLOBS_REQUEST {
            return Err(ConversionError::TooManyItems {
                field: "blobs",
                max: MAX_BLOBS_REQUEST,
                actual: value.len(),
            })
        }

        let entries = value
            .into_iter()
            .map(|value| match value {
                Some(contents) => BlobEntry { available: true, contents },
                None => BlobEntry { available: false, contents: zero_blob_v2() },
            })
            .collect();
        Ok(Self { entries })
    }
}

impl TryFrom<Vec<Option<BlobCellsAndProofsV1>>> for BlobsV4Response {
    type Error = ConversionError;

    fn try_from(value: Vec<Option<BlobCellsAndProofsV1>>) -> Result<Self, Self::Error> {
        if value.len() > MAX_BLOBS_REQUEST {
            return Err(ConversionError::TooManyItems {
                field: "blobs",
                max: MAX_BLOBS_REQUEST,
                actual: value.len(),
            })
        }

        let entries = value
            .into_iter()
            .map(|value| match value {
                Some(contents) => BlobEntry {
                    available: true,
                    contents: BlobCellsAndProofs {
                        blob_cells: contents.blob_cells.into_iter().map(Optional::from).collect(),
                        proofs: contents.proofs.into_iter().map(Optional::from).collect(),
                    },
                },
                None => BlobEntry { available: false, contents: BlobCellsAndProofs::default() },
            })
            .collect();
        Ok(Self { entries })
    }
}

/// A bounded trie-node byte list in an [`ExecutionWitnessV1`].
pub type WitnessNodeV1 = Vec<u8>;

/// A bounded contract-code byte list in an [`ExecutionWitnessV1`].
pub type WitnessCodeV1 = Vec<u8>;

/// A bounded RLP-encoded header byte list in an [`ExecutionWitnessV1`].
pub type WitnessHeaderV1 = Vec<u8>;

/// Canonical execution witness for `POST /payloads/witness`.
///
/// `state` and `codes` are produced in lexicographic ascending byte order. `headers` are
/// RLP-encoded and ordered by ascending block number; consecutive headers must be parent-linked.
/// These ordering rules are producer-side requirements from the execution-specs witness builder.
///
/// This is a REST-SSZ wire container, not the JSON-RPC debug witness shape.
#[derive(Clone, Debug, Default, PartialEq, Eq, ssz_derive::Encode)]
pub struct ExecutionWitnessV1 {
    /// Hashed trie-node preimages required during execution and state-root recomputation.
    pub state: Vec<WitnessNodeV1>,
    /// Contract bytecode preimages created or accessed during execution.
    pub codes: Vec<WitnessCodeV1>,
    /// RLP-encoded ancestor headers used for pre-state and `BLOCKHASH` correctness proofs.
    pub headers: Vec<WitnessHeaderV1>,
}

/// Canonical execution witness for `POST /payloads/witness`.
pub type ExecutionWitness = ExecutionWitnessV1;

/// REST-SSZ response for `POST /payloads/witness`.
///
/// The witness uses the Engine REST-SSZ `Optional[T]` encoding from execution-apis and is present
/// only when the payload status is `VALID`.
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
        Self { payload_status, witness: witness.into() }
    }
}

/// Backwards-compatible alias for the experimental witness response name.
pub type NewPayloadWithWitnessResponseV1 = PayloadStatusWithWitness;

impl ssz::Decode for ExecutionWitnessV1 {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        #[derive(ssz_derive::Decode)]
        struct Raw {
            state: Vec<Vec<u8>>,
            codes: Vec<Vec<u8>>,
            headers: Vec<Vec<u8>>,
        }

        let raw = Raw::from_ssz_bytes(bytes)?;
        validate_witness_list("state", &raw.state)?;
        validate_witness_list("codes", &raw.codes)?;
        validate_witness_list("headers", &raw.headers)?;
        Ok(Self { state: raw.state, codes: raw.codes, headers: raw.headers })
    }
}

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
        if response.witness.is_some() &&
            !matches!(response.payload_status.status, PayloadStatusEnum::Valid)
        {
            return Err(ssz::DecodeError::BytesInvalid(
                "execution witness is only valid for VALID payload status".into(),
            ))
        }
        Ok(response)
    }
}

fn validate_witness_list(name: &'static str, values: &[Vec<u8>]) -> Result<(), ssz::DecodeError> {
    if values.len() > MAX_EXECUTION_WITNESS_ENTRIES {
        return Err(ssz::DecodeError::BytesInvalid(format!(
            "too many witness {name} entries: expected at most {MAX_EXECUTION_WITNESS_ENTRIES}, got {}",
            values.len()
        )))
    }
    if let Some(value) = values.iter().find(|value| value.len() > MAX_EXECUTION_WITNESS_BYTES) {
        return Err(ssz::DecodeError::BytesInvalid(format!(
            "witness {name} entry too large: expected at most {MAX_EXECUTION_WITNESS_BYTES} bytes, got {}",
            value.len()
        )))
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip4895::Withdrawal;
    use alloy_primitives::{Address, Bloom, Bytes};
    use ssz::{Decode, Encode};

    fn payload_v1() -> ExecutionPayloadV1 {
        ExecutionPayloadV1 {
            parent_hash: B256::repeat_byte(1),
            fee_recipient: Address::repeat_byte(2),
            state_root: B256::repeat_byte(3),
            receipts_root: B256::repeat_byte(4),
            logs_bloom: Bloom::repeat_byte(5),
            prev_randao: B256::repeat_byte(6),
            block_number: 7,
            gas_limit: 8,
            gas_used: 9,
            timestamp: 10,
            extra_data: Bytes::from_static(&[11, 12]),
            base_fee_per_gas: U256::from(13),
            block_hash: B256::repeat_byte(14),
            transactions: vec![Bytes::from_static(&[15, 16])],
        }
    }

    fn payload_v2() -> ExecutionPayloadV2 {
        ExecutionPayloadV2 { payload_inner: payload_v1(), withdrawals: vec![Withdrawal::default()] }
    }

    fn payload_v3() -> ExecutionPayloadV3 {
        ExecutionPayloadV3 { payload_inner: payload_v2(), blob_gas_used: 17, excess_blob_gas: 18 }
    }

    fn payload_v4() -> ExecutionPayloadV4 {
        ExecutionPayloadV4 {
            payload_inner: payload_v3(),
            block_access_list: Bytes::from_static(&[19, 20]),
            slot_number: 21,
        }
    }

    fn attributes_cancun() -> PayloadAttributesCancun {
        PayloadAttributesCancun {
            timestamp: 1,
            prev_randao: B256::repeat_byte(2),
            suggested_fee_recipient: Address::repeat_byte(3),
            withdrawals: vec![Withdrawal::default()],
            parent_beacon_block_root: B256::repeat_byte(4),
        }
    }

    fn state() -> ForkchoiceState {
        ForkchoiceState {
            head_block_hash: B256::repeat_byte(1),
            safe_block_hash: B256::repeat_byte(2),
            finalized_block_hash: B256::repeat_byte(3),
        }
    }

    fn assert_roundtrip<T>(value: &T)
    where
        T: Encode + Decode + PartialEq + core::fmt::Debug,
    {
        assert_eq!(T::from_ssz_bytes(&value.as_ssz_bytes()).unwrap(), *value);
    }

    #[test]
    fn execution_payload_envelopes_roundtrip() {
        assert_roundtrip(&ExecutionPayloadEnvelopeParis { payload: payload_v1() });
        assert_roundtrip(&ExecutionPayloadEnvelopeShanghai { payload: payload_v2() });
        assert_roundtrip(&ExecutionPayloadEnvelopeCancun {
            payload: payload_v3(),
            parent_beacon_block_root: B256::repeat_byte(1),
        });
        assert_roundtrip(&ExecutionPayloadEnvelopePrague {
            payload: payload_v3(),
            parent_beacon_block_root: B256::repeat_byte(1),
            execution_requests: Requests::from_requests([Bytes::from_static(&[2, 3])]),
        });
        assert_roundtrip(&ExecutionPayloadEnvelopeOsaka {
            payload: payload_v3(),
            parent_beacon_block_root: B256::repeat_byte(1),
            execution_requests: Requests::from_requests([Bytes::from_static(&[2, 3])]),
        });
        assert_roundtrip(&ExecutionPayloadEnvelopeAmsterdam {
            payload: payload_v4(),
            parent_beacon_block_root: B256::repeat_byte(1),
            execution_requests: Requests::from_requests([Bytes::from_static(&[2, 3])]),
        });
    }

    #[test]
    fn paris_submission_is_a_single_field_container() {
        let payload = payload_v1();
        let payload_bytes = payload.as_ssz_bytes();
        let envelope = ExecutionPayloadEnvelopeParis { payload };
        let encoded = envelope.as_ssz_bytes();
        assert_eq!(&encoded[..4], &4u32.to_le_bytes());
        assert_eq!(&encoded[4..], payload_bytes);
    }

    #[test]
    fn built_payloads_roundtrip() {
        assert_roundtrip(&BuiltPayloadParis { payload: payload_v1(), block_value: U256::from(1) });
        assert_roundtrip(&BuiltPayloadShanghai {
            payload: payload_v2(),
            block_value: U256::from(1),
        });
        assert_roundtrip(&BuiltPayloadCancun {
            execution_payload: payload_v3(),
            block_value: U256::from(1),
            blobs_bundle: BlobsBundleV1::empty(),
            should_override_builder: true,
        });
        assert_roundtrip(&BuiltPayloadPrague {
            payload: payload_v3(),
            block_value: U256::from(1),
            blobs_bundle: BlobsBundleV1::empty(),
            execution_requests: Requests::from_requests([Bytes::from_static(&[2, 3])]),
            should_override_builder: true,
        });
        assert_roundtrip(&BuiltPayloadOsaka {
            payload: payload_v3(),
            block_value: U256::from(1),
            blobs_bundle: BlobsBundleV2::empty(),
            execution_requests: Requests::from_requests([Bytes::from_static(&[2, 3])]),
            should_override_builder: true,
        });
        assert_roundtrip(&BuiltPayloadAmsterdam {
            payload: payload_v4(),
            block_value: U256::from(1),
            blobs_bundle: BlobsBundleV2::empty(),
            execution_requests: Requests::from_requests([Bytes::from_static(&[2, 3])]),
            should_override_builder: true,
        });
    }

    #[test]
    fn shanghai_built_payload_has_no_builder_override() {
        let payload = payload_v2();
        let payload_len = payload.ssz_bytes_len();
        let value = BuiltPayloadShanghai { payload, block_value: U256::from(1) };
        let encoded = value.as_ssz_bytes();

        assert_eq!(&encoded[..4], &36u32.to_le_bytes());
        assert_eq!(encoded.len(), 36 + payload_len);
    }

    #[test]
    fn legacy_built_payload_conversions_preserve_fields() {
        let shanghai = BuiltPayloadShanghai { payload: payload_v2(), block_value: U256::from(1) };
        let legacy = LegacyBuiltPayloadShanghai::from(shanghai.clone());
        assert_eq!(BuiltPayloadShanghai::try_from(legacy).unwrap(), shanghai);

        let prague = BuiltPayloadPrague {
            payload: payload_v3(),
            block_value: U256::from(2),
            blobs_bundle: BlobsBundleV1::empty(),
            execution_requests: Requests::from_requests([Bytes::from_static(&[3, 4])]),
            should_override_builder: true,
        };
        let legacy = LegacyBuiltPayloadPrague::from(prague.clone());
        assert_eq!(BuiltPayloadPrague::from(legacy), prague);

        let osaka = BuiltPayloadOsaka {
            payload: payload_v3(),
            block_value: U256::from(5),
            blobs_bundle: BlobsBundleV2::empty(),
            execution_requests: Requests::from_requests([Bytes::from_static(&[6, 7])]),
            should_override_builder: true,
        };
        let legacy = LegacyBuiltPayloadOsaka::from(osaka.clone());
        assert_eq!(BuiltPayloadOsaka::from(legacy), osaka);

        let amsterdam = BuiltPayloadAmsterdam {
            payload: payload_v4(),
            block_value: U256::from(8),
            blobs_bundle: BlobsBundleV2::empty(),
            execution_requests: Requests::from_requests([Bytes::from_static(&[9, 10])]),
            should_override_builder: true,
        };
        let legacy = LegacyBuiltPayloadAmsterdam::from(amsterdam.clone());
        assert_eq!(BuiltPayloadAmsterdam::from(legacy), amsterdam);
    }

    #[test]
    fn legacy_shanghai_built_payload_rejects_paris_payload() {
        let legacy = LegacyBuiltPayloadShanghai {
            execution_payload: ExecutionPayloadFieldV2::V1(payload_v1()),
            block_value: U256::from(1),
        };
        assert_eq!(
            BuiltPayloadShanghai::try_from(legacy),
            Err(BuiltPayloadConversionError::UnexpectedPayloadFork("Paris"))
        );
    }

    #[test]
    fn prague_requests_precede_should_override_builder() {
        let value = BuiltPayloadPrague {
            payload: payload_v3(),
            block_value: U256::from(1),
            blobs_bundle: BlobsBundleV1::empty(),
            execution_requests: Requests::from_requests([Bytes::from_static(&[0xaa, 0xbb])]),
            should_override_builder: true,
        };
        let encoded = value.as_ssz_bytes();
        assert_eq!(encoded[48], 1);
        assert_eq!(&encoded[encoded.len() - 2..], &[0xaa, 0xbb]);
    }

    #[test]
    fn forkchoice_updates_roundtrip() {
        let paris = PayloadAttributesParis {
            timestamp: 1,
            prev_randao: B256::repeat_byte(2),
            suggested_fee_recipient: Address::repeat_byte(3),
        };
        let shanghai = PayloadAttributesShanghai {
            timestamp: 1,
            prev_randao: B256::repeat_byte(2),
            suggested_fee_recipient: Address::repeat_byte(3),
            withdrawals: vec![Withdrawal::default()],
        };
        let amsterdam = PayloadAttributesAmsterdam {
            timestamp: 1,
            prev_randao: B256::repeat_byte(2),
            suggested_fee_recipient: Address::repeat_byte(3),
            withdrawals: vec![Withdrawal::default()],
            parent_beacon_block_root: B256::repeat_byte(4),
            slot_number: 5,
            target_gas_limit: 6,
        };
        assert_roundtrip(&ForkchoiceUpdateParis {
            forkchoice_state: state(),
            payload_attributes: Optional::some(paris),
        });
        assert_roundtrip(&ForkchoiceUpdateShanghai {
            forkchoice_state: state(),
            payload_attributes: Optional::some(shanghai),
        });
        assert_roundtrip(&ForkchoiceUpdateCancun {
            forkchoice_state: state(),
            payload_attributes: Optional::some(attributes_cancun()),
        });
        assert_roundtrip(&ForkchoiceUpdatePrague {
            forkchoice_state: state(),
            payload_attributes: Optional::some(attributes_cancun()),
        });
        assert_roundtrip(&ForkchoiceUpdateOsaka {
            forkchoice_state: state(),
            payload_attributes: Optional::some(attributes_cancun()),
        });
        assert_roundtrip(&ForkchoiceUpdateAmsterdam {
            forkchoice_state: state(),
            payload_attributes: Optional::some(amsterdam),
            custody_columns: Optional::some(B128::repeat_byte(0xa5)),
        });
    }

    #[test]
    fn payload_attributes_legacy_conversions_preserve_fork_shape() {
        let cancun = attributes_cancun();
        let legacy = LegacyPayloadAttributes::from(cancun.clone());
        assert_eq!(PayloadAttributesCancun::try_from(legacy).unwrap(), cancun);

        let amsterdam = PayloadAttributesAmsterdam {
            timestamp: 1,
            prev_randao: B256::repeat_byte(2),
            suggested_fee_recipient: Address::repeat_byte(3),
            withdrawals: vec![Withdrawal::default()],
            parent_beacon_block_root: B256::repeat_byte(4),
            slot_number: 5,
            target_gas_limit: 6,
        };
        let legacy = LegacyPayloadAttributes::from(amsterdam.clone());
        assert_eq!(PayloadAttributesAmsterdam::try_from(legacy).unwrap(), amsterdam);
    }

    #[test]
    fn payload_attributes_legacy_conversions_reject_loss() {
        let mut legacy = LegacyPayloadAttributes::default();
        assert_eq!(
            PayloadAttributesShanghai::try_from(legacy.clone()),
            Err(PayloadAttributesConversionError::MissingField("withdrawals"))
        );

        legacy.withdrawals = Some(vec![]);
        legacy.parent_beacon_block_root = Some(B256::ZERO);
        assert_eq!(
            PayloadAttributesShanghai::try_from(legacy),
            Err(PayloadAttributesConversionError::UnexpectedField("parent_beacon_block_root"))
        );
    }

    #[test]
    fn every_payload_status_roundtrips() {
        for status in [
            PayloadStatusEnum::Valid,
            PayloadStatusEnum::Invalid { validation_error: "invalid".into() },
            PayloadStatusEnum::Syncing,
            PayloadStatusEnum::Accepted,
        ] {
            let validation_error = legacy_validation_error(&status).unwrap();
            let value = PayloadStatus {
                status,
                latest_valid_hash: Optional::some(B256::ZERO),
                validation_error,
            };
            assert_eq!(PayloadStatus::from_ssz_bytes(&value.as_ssz_bytes()).unwrap(), value);
        }
    }

    #[test]
    fn payload_status_preserves_absent_invalid_validation_error() {
        let mut bytes = Vec::new();
        let mut encoder = ssz::SszEncoder::container(&mut bytes, 9);
        encoder.append(&1u8);
        encoder.append(&Optional::<B256>::none());
        encoder.append(&Optional::<ErrorBytes>::none());
        encoder.finalize();

        let decoded = PayloadStatus::from_ssz_bytes(&bytes).unwrap();
        assert!(decoded.validation_error.is_none());
        assert_eq!(decoded.as_ssz_bytes(), bytes);
    }

    #[test]
    fn payload_status_rejects_non_invalid_validation_error() {
        let mut bytes = Vec::new();
        let mut encoder = ssz::SszEncoder::container(&mut bytes, 9);
        encoder.append(&0u8);
        encoder.append(&Optional::<B256>::none());
        encoder.append(&Optional::some(Vec::<u8>::new()));
        encoder.finalize();
        assert!(PayloadStatus::from_ssz_bytes(&bytes).is_err());
    }

    #[test]
    fn payload_status_legacy_conversion_rejects_oversized_error() {
        assert!(PayloadStatus::try_from(LegacyPayloadStatus {
            status: PayloadStatusEnum::Invalid { validation_error: "x".repeat(1025) },
            latest_valid_hash: None,
        })
        .is_err());
    }

    #[test]
    fn forkchoice_response_distinguishes_absent_and_zero_payload_id() {
        let status = PayloadStatus {
            status: PayloadStatusEnum::Valid,
            latest_valid_hash: Optional::none(),
            validation_error: Optional::none(),
        };
        let none = ForkchoiceUpdateResponse {
            payload_status: status.clone(),
            payload_id: Optional::none(),
        };
        let zero = ForkchoiceUpdateResponse {
            payload_status: status,
            payload_id: Optional::some(PayloadId::default()),
        };

        assert_ne!(none.as_ssz_bytes(), zero.as_ssz_bytes());
        assert_roundtrip(&none);
        assert_roundtrip(&zero);
    }

    #[test]
    fn forkchoice_conversion_rejects_accepted() {
        let legacy = LegacyForkchoice::from_status(PayloadStatusEnum::Accepted);
        assert_eq!(
            ForkchoiceUpdateResponse::try_from(legacy),
            Err(ConversionError::AcceptedForkchoice)
        );
    }

    fn blob_v2(byte: u8) -> BlobAndProofV2 {
        BlobAndProofV2 {
            blob: Box::new(Blob::repeat_byte(byte)),
            proofs: vec![Bytes48::repeat_byte(byte)],
        }
    }

    #[test]
    fn blob_requests_are_single_field_containers() {
        let request = BlobsV1Request { versioned_hashes: vec![B256::repeat_byte(0x42)] };
        let encoded = request.as_ssz_bytes();

        assert_eq!(&encoded[..4], &4u32.to_le_bytes());
        assert_eq!(&encoded[4..], B256::repeat_byte(0x42).as_slice());
        assert_eq!(BlobsV1Request::from_ssz_bytes(&encoded).unwrap(), request);

        let _: BlobsV2Request = BlobsV2Request::from_ssz_bytes(&encoded).unwrap();
        let _: BlobsV3Request = BlobsV3Request::from_ssz_bytes(&encoded).unwrap();
    }

    #[test]
    fn blob_v4_request_roundtrips_bitvector() {
        let request = BlobsV4Request {
            versioned_hashes: vec![B256::repeat_byte(0x11)],
            indices_bitarray: B128::repeat_byte(0xa5),
        };

        assert_roundtrip(&request);
    }

    #[test]
    fn blob_response_conversions_preserve_availability_and_order() {
        let v1 = BlobsV1Response::try_from(vec![None]).unwrap();
        assert!(!v1.entries[0].available);
        assert_eq!(v1.entries[0].contents, zero_blob_v1());

        let v2 = BlobsV2Response::try_from(vec![blob_v2(1), blob_v2(2)]).unwrap();
        assert!(v2.entries.iter().all(|entry| entry.available));

        let v3 = BlobsV3Response::try_from(vec![Some(blob_v2(1)), None, Some(blob_v2(3))]).unwrap();
        assert_eq!(
            v3.entries.iter().map(|entry| entry.available).collect::<Vec<_>>(),
            [true, false, true]
        );
        assert_eq!(v3.entries[2].contents.blob.as_slice(), Blob::repeat_byte(3).as_slice());

        let legacy_partial = BlobCellsAndProofsV1 {
            blob_cells: vec![Some(Cell::repeat_byte(1)), None],
            proofs: vec![Some(Bytes48::repeat_byte(2)), None],
        };
        let v4 = BlobsV4Response::try_from(vec![None, Some(legacy_partial)]).unwrap();
        assert!(!v4.entries[0].available);
        assert!(v4.entries[1].available);
        assert!(v4.entries[1].contents.blob_cells[0].is_some());
        assert!(v4.entries[1].contents.proofs[1].is_none());
    }

    #[test]
    fn blob_cells_and_proofs_uses_rest_optional() {
        let value = BlobCellsAndProofs {
            blob_cells: vec![Optional::some(Cell::repeat_byte(1))],
            proofs: vec![Optional::some(Bytes48::repeat_byte(2))],
        };
        let encoded = value.as_ssz_bytes();

        assert_eq!(BlobCellsAndProofs::from_ssz_bytes(&encoded).unwrap(), value);
        assert!(!encoded[8..].starts_with(&[1, 0, 0, 0]));
    }

    #[test]
    fn payload_body_requests_are_single_field_containers() {
        let request = BodiesByHashRequest { block_hashes: vec![B256::repeat_byte(0x33)] };
        let encoded = request.as_ssz_bytes();

        assert_eq!(&encoded[..4], &4u32.to_le_bytes());
        assert_eq!(&encoded[4..], B256::repeat_byte(0x33).as_slice());
        assert_eq!(BodiesByHashRequest::from_ssz_bytes(&encoded).unwrap(), request);
    }

    #[test]
    fn payload_body_responses_preserve_availability() {
        let legacy = LegacyExecutionPayloadBodyV1 {
            transactions: vec![Bytes::from_static(&[1, 2, 3])],
            withdrawals: Some(vec![Withdrawal::default()]),
        };
        let response =
            BodiesResponseShanghai::from_optional_bodies(vec![Some(legacy), None], |body| {
                ExecutionPayloadBodyShanghai::try_from(body).ok()
            })
            .unwrap();

        assert!(response.entries[0].available);
        assert!(!response.entries[1].available);
        assert_roundtrip(&response);
    }

    #[test]
    fn witness_response_roundtrips_when_status_is_valid() {
        let payload_status = PayloadStatus {
            status: PayloadStatusEnum::Valid,
            latest_valid_hash: Optional::none(),
            validation_error: Optional::none(),
        };
        let witness = ExecutionWitnessV1 {
            state: vec![vec![1, 2, 3]],
            codes: vec![vec![4, 5]],
            headers: vec![vec![6]],
        };
        let response = PayloadStatusWithWitness::new(payload_status, Some(witness));

        assert_roundtrip(&response);
    }

    #[test]
    fn witness_response_rejects_non_valid_status_with_witness() {
        let payload_status = PayloadStatus {
            status: PayloadStatusEnum::Syncing,
            latest_valid_hash: Optional::none(),
            validation_error: Optional::none(),
        };
        let response =
            PayloadStatusWithWitness::new(payload_status, Some(ExecutionWitnessV1::default()));

        assert!(PayloadStatusWithWitness::from_ssz_bytes(&response.as_ssz_bytes()).is_err());
    }

    #[test]
    fn optional_rejects_more_than_one_value() {
        assert!(Optional::<B256>::from_ssz_bytes(&[0; 64]).is_err());
    }
}
