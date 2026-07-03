//! Experimental Engine API v2 REST-SSZ wire types.
//!
//! These types intentionally live apart from the legacy JSON-RPC Engine API types because their
//! SSZ encodings are not always wire-compatible. This module contains the shared endpoint
//! containers and fork-specific payload containers from
//! [execution-apis PR #793](https://github.com/ethereum/execution-apis/pull/793), plus the
//! experimental payload-with-witness response type that extends the same REST-SSZ model.

use alloy_eips::eip7685::Requests;
use alloy_primitives::{B256, U256};
use alloy_rpc_types_engine::{
    BlobsBundleV1, BlobsBundleV2, ExecutionPayloadEnvelopeV2 as LegacyBuiltPayloadShanghai,
    ExecutionPayloadEnvelopeV4 as LegacyBuiltPayloadPrague,
    ExecutionPayloadEnvelopeV5 as LegacyBuiltPayloadOsaka,
    ExecutionPayloadEnvelopeV6 as LegacyBuiltPayloadAmsterdam, ExecutionPayloadFieldV2,
    ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3, ExecutionPayloadV4,
};

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

/// This structure maps to the Engine API v2 REST-SSZ payload-submission request for Amsterdam.
///
/// This is distinct from the legacy [`alloy_rpc_types_engine::ExecutionPayloadEnvelopeV6`], which
/// is the `engine_getPayloadV6` response.
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
}
