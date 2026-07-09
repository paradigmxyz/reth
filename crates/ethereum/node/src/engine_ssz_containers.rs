//! Experimental Engine API v2 REST-SSZ wire types.
//!
//! These types intentionally live apart from the legacy JSON-RPC Engine API types because their
//! SSZ encodings are not always wire-compatible. This module contains the shared endpoint
//! containers and fork-specific payload containers from
//! [execution-apis PR #793](https://github.com/ethereum/execution-apis/pull/793), plus the
//! experimental payload-with-witness response type that extends the same REST-SSZ model.

use alloy_eips::{eip4895::Withdrawal, eip7685::Requests};
use alloy_primitives::{Address, B128, B256, U256};
use alloy_rpc_types_engine::{
    BlobsBundleV1, BlobsBundleV2, ExecutionPayloadEnvelopeV2 as LegacyBuiltPayloadShanghai,
    ExecutionPayloadEnvelopeV4 as LegacyBuiltPayloadPrague,
    ExecutionPayloadEnvelopeV5 as LegacyBuiltPayloadOsaka,
    ExecutionPayloadEnvelopeV6 as LegacyBuiltPayloadAmsterdam, ExecutionPayloadFieldV2,
    ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3, ExecutionPayloadV4,
    ForkchoiceState, ForkchoiceUpdated as LegacyForkchoice,
    PayloadAttributes as LegacyPayloadAttributes, PayloadId, PayloadStatus as LegacyPayloadStatus,
    PayloadStatusEnum,
};

type ErrorBytes = Vec<u8>;

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
}

impl core::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ErrorBytesTooLong => f.write_str("payload validation error is too long"),
            Self::AcceptedForkchoice => {
                f.write_str("ACCEPTED is not valid in a forkchoice response")
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

    #[test]
    fn optional_rejects_more_than_one_value() {
        assert!(Optional::<B256>::from_ssz_bytes(&[0; 64]).is_err());
    }
}
