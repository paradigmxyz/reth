pub use crate::Withdrawal;
use reth_primitives::{
    kzg::{Blob, Bytes48},
    Address, BlobTransactionSidecar, Bloom, Bytes, SealedBlock, B256, B64, U256, U64,
};
use serde::{ser::SerializeMap, Deserialize, Serialize, Serializer};

/// The execution payload body response that allows for `null` values.
pub type ExecutionPayloadBodiesV1 = Vec<Option<ExecutionPayloadBodyV1>>;

/// And 8-byte identifier for an execution payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct PayloadId(B64);

// === impl PayloadId ===

impl PayloadId {
    /// Creates a new payload id from the given identifier.
    pub fn new(id: [u8; 8]) -> Self {
        Self(B64::from(id))
    }
}

impl std::fmt::Display for PayloadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// This represents the `executionPayload` field in the return value of `engine_getPayloadV2`,
/// specified as:
///
///  - `executionPayload`: `ExecutionPayloadV1` | `ExecutionPayloadV2` where:
///    - `ExecutionPayloadV1` **MUST** be returned if the payload `timestamp` is lower than the
///    Shanghai timestamp
///    - `ExecutionPayloadV2` **MUST** be returned if the payload `timestamp` is greater or equal
///    to the Shanghai timestamp
///
/// See:
/// <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/shanghai.md#response>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExecutionPayloadFieldV2 {
    /// V1 payload
    V1(ExecutionPayloadV1),
    /// V2 payload
    V2(ExecutionPayloadV2),
}

impl ExecutionPayloadFieldV2 {
    /// Returns the inner [ExecutionPayloadV1]
    pub fn into_v1_payload(self) -> ExecutionPayloadV1 {
        match self {
            ExecutionPayloadFieldV2::V1(payload) => payload,
            ExecutionPayloadFieldV2::V2(payload) => payload.payload_inner,
        }
    }
}

/// This is the input to `engine_newPayloadV2`, which may or may not have a withdrawals field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionPayloadInputV2 {
    /// The V1 execution payload
    #[serde(flatten)]
    pub execution_payload: ExecutionPayloadV1,
    /// The payload withdrawals
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawals: Option<Vec<Withdrawal>>,
}

/// This structure maps for the return value of `engine_getPayload` of the beacon chain spec, for
/// V2.
///
/// See also:
/// <https://github.com/ethereum/execution-apis/blob/main/src/engine/shanghai.md#engine_getpayloadv2>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPayloadEnvelopeV2 {
    /// Execution payload, which could be either V1 or V2
    ///
    /// V1 (_NO_ withdrawals) MUST be returned if the payload timestamp is lower than the Shanghai
    /// timestamp
    ///
    /// V2 (_WITH_ withdrawals) MUST be returned if the payload timestamp is greater or equal to
    /// the Shanghai timestamp
    pub execution_payload: ExecutionPayloadFieldV2,
    /// The expected value to be received by the feeRecipient in wei
    pub block_value: U256,
}

impl ExecutionPayloadEnvelopeV2 {
    /// Returns the [ExecutionPayload] for the `engine_getPayloadV1` endpoint
    pub fn into_v1_payload(self) -> ExecutionPayloadV1 {
        self.execution_payload.into_v1_payload()
    }
}

/// This structure maps for the return value of `engine_getPayload` of the beacon chain spec, for
/// V3.
///
/// See also:
/// <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#response-2>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPayloadEnvelopeV3 {
    /// Execution payload V3
    pub execution_payload: ExecutionPayloadV3,
    /// The expected value to be received by the feeRecipient in wei
    pub block_value: U256,
    /// The blobs, commitments, and proofs associated with the executed payload.
    pub blobs_bundle: BlobsBundleV1,
    /// Introduced in V3, this represents a suggestion from the execution layer if the payload
    /// should be used instead of an externally provided one.
    pub should_override_builder: bool,
}

/// This structure maps on the ExecutionPayload structure of the beacon chain spec.
///
/// See also: <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#executionpayloadv1>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPayloadV1 {
    pub parent_hash: B256,
    pub fee_recipient: Address,
    pub state_root: B256,
    pub receipts_root: B256,
    pub logs_bloom: Bloom,
    pub prev_randao: B256,
    pub block_number: U64,
    pub gas_limit: U64,
    pub gas_used: U64,
    pub timestamp: U64,
    pub extra_data: Bytes,
    pub base_fee_per_gas: U256,
    pub block_hash: B256,
    pub transactions: Vec<Bytes>,
}

impl From<SealedBlock> for ExecutionPayloadV1 {
    fn from(value: SealedBlock) -> Self {
        let transactions = value
            .body
            .iter()
            .map(|tx| {
                let mut encoded = Vec::new();
                tx.encode_enveloped(&mut encoded);
                encoded.into()
            })
            .collect();
        ExecutionPayloadV1 {
            parent_hash: value.parent_hash,
            fee_recipient: value.beneficiary,
            state_root: value.state_root,
            receipts_root: value.receipts_root,
            logs_bloom: value.logs_bloom,
            prev_randao: value.mix_hash,
            block_number: U64::from(value.number),
            gas_limit: U64::from(value.gas_limit),
            gas_used: U64::from(value.gas_used),
            timestamp: U64::from(value.timestamp),
            extra_data: value.extra_data.clone(),
            base_fee_per_gas: U256::from(value.base_fee_per_gas.unwrap_or_default()),
            block_hash: value.hash(),
            transactions,
        }
    }
}

/// This structure maps on the ExecutionPayloadV2 structure of the beacon chain spec.
///
/// See also: <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/shanghai.md#executionpayloadv2>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionPayloadV2 {
    /// Inner V1 payload
    #[serde(flatten)]
    pub payload_inner: ExecutionPayloadV1,

    /// Array of [`Withdrawal`] enabled with V2
    /// See <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/shanghai.md#executionpayloadv2>
    pub withdrawals: Vec<Withdrawal>,
}

impl ExecutionPayloadV2 {
    /// Returns the timestamp for the execution payload.
    pub fn timestamp(&self) -> u64 {
        self.payload_inner.timestamp.to()
    }
}

/// This structure maps on the ExecutionPayloadV3 structure of the beacon chain spec.
///
/// See also: <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/shanghai.md#executionpayloadv2>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPayloadV3 {
    /// Inner V2 payload
    #[serde(flatten)]
    pub payload_inner: ExecutionPayloadV2,

    /// Array of [`U64`] representing blob gas used, enabled with V3
    /// See <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#ExecutionPayloadV3>
    pub blob_gas_used: U64,
    /// Array of [`U64`] representing excess blob gas, enabled with V3
    /// See <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#ExecutionPayloadV3>
    pub excess_blob_gas: U64,
}

impl ExecutionPayloadV3 {
    /// Returns the withdrawals for the payload.
    pub fn withdrawals(&self) -> &Vec<Withdrawal> {
        &self.payload_inner.withdrawals
    }

    /// Returns the timestamp for the payload.
    pub fn timestamp(&self) -> u64 {
        self.payload_inner.payload_inner.timestamp.to()
    }
}

/// This includes all bundled blob related data of an executed payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobsBundleV1 {
    pub commitments: Vec<Bytes48>,
    pub proofs: Vec<Bytes48>,
    pub blobs: Vec<Blob>,
}

impl From<Vec<BlobTransactionSidecar>> for BlobsBundleV1 {
    fn from(sidecars: Vec<BlobTransactionSidecar>) -> Self {
        let (commitments, proofs, blobs) = sidecars.into_iter().fold(
            (Vec::new(), Vec::new(), Vec::new()),
            |(mut commitments, mut proofs, mut blobs), sidecar| {
                commitments.extend(sidecar.commitments);
                proofs.extend(sidecar.proofs);
                blobs.extend(sidecar.blobs);
                (commitments, proofs, blobs)
            },
        );
        Self { commitments, proofs, blobs }
    }
}

impl BlobsBundleV1 {
    /// Take `len` blob data from the bundle.
    ///
    /// # Panics
    ///
    /// If len is more than the blobs bundle len.
    pub fn take(&mut self, len: usize) -> (Vec<Bytes48>, Vec<Bytes48>, Vec<Blob>) {
        (
            self.commitments.drain(0..len).collect(),
            self.proofs.drain(0..len).collect(),
            self.blobs.drain(0..len).collect(),
        )
    }
}

/// An execution payload, which can be either [ExecutionPayloadV1], [ExecutionPayloadV2], or
/// [ExecutionPayloadV3].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionPayload {
    /// V1 payload
    V1(ExecutionPayloadV1),
    /// V2 payload
    V2(ExecutionPayloadV2),
    /// V3 payload
    V3(ExecutionPayloadV3),
}

impl ExecutionPayload {
    /// Returns the withdrawals for the payload.
    pub fn withdrawals(&self) -> Option<&Vec<Withdrawal>> {
        match self {
            ExecutionPayload::V1(_) => None,
            ExecutionPayload::V2(payload) => Some(&payload.withdrawals),
            ExecutionPayload::V3(payload) => Some(payload.withdrawals()),
        }
    }

    /// Returns the timestamp for the payload.
    pub fn timestamp(&self) -> u64 {
        match self {
            ExecutionPayload::V1(payload) => payload.timestamp.to(),
            ExecutionPayload::V2(payload) => payload.timestamp(),
            ExecutionPayload::V3(payload) => payload.timestamp(),
        }
    }

    /// Returns the parent hash for the payload.
    pub fn parent_hash(&self) -> B256 {
        match self {
            ExecutionPayload::V1(payload) => payload.parent_hash,
            ExecutionPayload::V2(payload) => payload.payload_inner.parent_hash,
            ExecutionPayload::V3(payload) => payload.payload_inner.payload_inner.parent_hash,
        }
    }

    /// Returns the block hash for the payload.
    pub fn block_hash(&self) -> B256 {
        match self {
            ExecutionPayload::V1(payload) => payload.block_hash,
            ExecutionPayload::V2(payload) => payload.payload_inner.block_hash,
            ExecutionPayload::V3(payload) => payload.payload_inner.payload_inner.block_hash,
        }
    }

    /// Returns the block number for this payload.
    pub fn block_number(&self) -> u64 {
        match self {
            ExecutionPayload::V1(payload) => payload.block_number.to(),
            ExecutionPayload::V2(payload) => payload.payload_inner.block_number.to(),
            ExecutionPayload::V3(payload) => payload.payload_inner.payload_inner.block_number.to(),
        }
    }
}

impl From<ExecutionPayloadV1> for ExecutionPayload {
    fn from(payload: ExecutionPayloadV1) -> Self {
        Self::V1(payload)
    }
}

impl From<ExecutionPayloadV2> for ExecutionPayload {
    fn from(payload: ExecutionPayloadV2) -> Self {
        Self::V2(payload)
    }
}

impl From<ExecutionPayloadV3> for ExecutionPayload {
    fn from(payload: ExecutionPayloadV3) -> Self {
        Self::V3(payload)
    }
}

/// Error that can occur when handling payloads.
#[derive(thiserror::Error, Debug)]
pub enum PayloadError {
    /// Invalid payload extra data.
    #[error("Invalid payload extra data: {0}")]
    ExtraData(Bytes),
    /// Invalid payload base fee.
    #[error("Invalid payload base fee: {0}")]
    BaseFee(U256),
    /// Invalid payload blob gas used.
    #[error("Invalid payload blob gas used: {0}")]
    BlobGasUsed(U256),
    /// Invalid payload excess blob gas.
    #[error("Invalid payload excess blob gas: {0}")]
    ExcessBlobGas(U256),
    /// Pre-cancun Payload has blob transactions.
    #[error("Invalid payload, pre-Cancun payload has blob transactions")]
    PreCancunBlockWithBlobTransactions,
    /// Invalid payload block hash.
    #[error("blockhash mismatch, want {consensus}, got {execution}")]
    BlockHash {
        /// The block hash computed from the payload.
        execution: B256,
        /// The block hash provided with the payload.
        consensus: B256,
    },
    /// Expected blob versioned hashes do not match the given transactions.
    #[error("Expected blob versioned hashes do not match the given transactions")]
    InvalidVersionedHashes,
    /// Encountered decoding error.
    #[error(transparent)]
    Decode(#[from] alloy_rlp::Error),
}

impl PayloadError {
    /// Returns `true` if the error is caused by invalid extra data.
    pub fn is_block_hash_mismatch(&self) -> bool {
        matches!(self, PayloadError::BlockHash { .. })
    }
}

/// This structure contains a body of an execution payload.
///
/// See also: <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#executionpayloadbodyv1>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPayloadBodyV1 {
    /// Enveloped encoded transactions.
    pub transactions: Vec<Bytes>,
    /// All withdrawals in the block.
    ///
    /// Will always be `None` if pre shanghai.
    pub withdrawals: Option<Vec<Withdrawal>>,
}

/// This structure contains the attributes required to initiate a payload build process in the
/// context of an `engine_forkchoiceUpdated` call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadAttributes {
    /// Value for the `timestamp` field of the new payload
    pub timestamp: U64,
    /// Value for the `prevRandao` field of the new payload
    pub prev_randao: B256,
    /// Suggested value for the `feeRecipient` field of the new payload
    pub suggested_fee_recipient: Address,
    /// Array of [`Withdrawal`] enabled with V2
    /// See <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#payloadattributesv2>
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawals: Option<Vec<Withdrawal>>,
    /// Root of the parent beacon block enabled with V3.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/cancun.md#payloadattributesv3>
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_beacon_block_root: Option<B256>,
}

/// This structure contains the result of processing a payload or fork choice update.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadStatus {
    #[serde(flatten)]
    pub status: PayloadStatusEnum,
    /// Hash of the most recent valid block in the branch defined by payload and its ancestors
    pub latest_valid_hash: Option<B256>,
}

impl PayloadStatus {
    pub fn new(status: PayloadStatusEnum, latest_valid_hash: Option<B256>) -> Self {
        Self { status, latest_valid_hash }
    }

    pub fn from_status(status: PayloadStatusEnum) -> Self {
        Self { status, latest_valid_hash: None }
    }

    pub fn with_latest_valid_hash(mut self, latest_valid_hash: B256) -> Self {
        self.latest_valid_hash = Some(latest_valid_hash);
        self
    }

    pub fn maybe_latest_valid_hash(mut self, latest_valid_hash: Option<B256>) -> Self {
        self.latest_valid_hash = latest_valid_hash;
        self
    }

    /// Returns true if the payload status is syncing.
    pub fn is_syncing(&self) -> bool {
        self.status.is_syncing()
    }

    /// Returns true if the payload status is valid.
    pub fn is_valid(&self) -> bool {
        self.status.is_valid()
    }

    /// Returns true if the payload status is invalid.
    pub fn is_invalid(&self) -> bool {
        self.status.is_invalid()
    }
}

impl std::fmt::Display for PayloadStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PayloadStatus {{status: {}, latestValidHash: {:?} }}",
            self.status, self.latest_valid_hash
        )
    }
}

impl Serialize for PayloadStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("status", self.status.as_str())?;
        map.serialize_entry("latestValidHash", &self.latest_valid_hash)?;
        map.serialize_entry("validationError", &self.status.validation_error())?;
        map.end()
    }
}

impl From<PayloadError> for PayloadStatusEnum {
    fn from(error: PayloadError) -> Self {
        PayloadStatusEnum::Invalid { validation_error: error.to_string() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PayloadStatusEnum {
    /// VALID is returned by the engine API in the following calls:
    ///   - newPayload:       if the payload was already known or was just validated and executed
    ///   - forkchoiceUpdate: if the chain accepted the reorg (might ignore if it's stale)
    Valid,

    /// INVALID is returned by the engine API in the following calls:
    ///   - newPayload:       if the payload failed to execute on top of the local chain
    ///   - forkchoiceUpdate: if the new head is unknown, pre-merge, or reorg to it fails
    Invalid {
        #[serde(rename = "validationError")]
        validation_error: String,
    },

    /// SYNCING is returned by the engine API in the following calls:
    ///   - newPayload:       if the payload was accepted on top of an active sync
    ///   - forkchoiceUpdate: if the new head was seen before, but not part of the chain
    Syncing,

    /// ACCEPTED is returned by the engine API in the following calls:
    ///   - newPayload: if the payload was accepted, but not processed (side chain)
    Accepted,
}

impl PayloadStatusEnum {
    /// Returns the string representation of the payload status.
    pub fn as_str(&self) -> &'static str {
        match self {
            PayloadStatusEnum::Valid => "VALID",
            PayloadStatusEnum::Invalid { .. } => "INVALID",
            PayloadStatusEnum::Syncing => "SYNCING",
            PayloadStatusEnum::Accepted => "ACCEPTED",
        }
    }

    /// Returns the validation error if the payload status is invalid.
    pub fn validation_error(&self) -> Option<&str> {
        match self {
            PayloadStatusEnum::Invalid { validation_error } => Some(validation_error),
            _ => None,
        }
    }

    /// Returns true if the payload status is syncing.
    pub fn is_syncing(&self) -> bool {
        matches!(self, PayloadStatusEnum::Syncing)
    }

    /// Returns true if the payload status is valid.
    pub fn is_valid(&self) -> bool {
        matches!(self, PayloadStatusEnum::Valid)
    }

    /// Returns true if the payload status is invalid.
    pub fn is_invalid(&self) -> bool {
        matches!(self, PayloadStatusEnum::Invalid { .. })
    }
}

impl std::fmt::Display for PayloadStatusEnum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PayloadStatusEnum::Invalid { validation_error } => {
                f.write_str(self.as_str())?;
                f.write_str(": ")?;
                f.write_str(validation_error.as_str())
            }
            _ => f.write_str(self.as_str()),
        }
    }
}

/// Various errors that can occur when validating a payload or forkchoice update.
///
/// This is intended for the [PayloadStatusEnum::Invalid] variant.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PayloadValidationError {
    /// Thrown when a forkchoice update's head links to a previously rejected payload.
    #[error("links to previously rejected block")]
    LinksToRejectedPayload,
    /// Thrown when a new payload contains a wrong block number.
    #[error("invalid block number")]
    InvalidBlockNumber,
    /// Thrown when a new payload contains a wrong state root
    #[error("invalid merkle root: (remote: {remote:?} local: {local:?})")]
    InvalidStateRoot {
        /// The state root of the payload we received from remote (CL)
        remote: B256,
        /// The state root of the payload that we computed locally.
        local: B256,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_payload_status() {
        let s = r#"{"status":"SYNCING","latestValidHash":null,"validationError":null}"#;
        let status: PayloadStatus = serde_json::from_str(s).unwrap();
        assert_eq!(status.status, PayloadStatusEnum::Syncing);
        assert!(status.latest_valid_hash.is_none());
        assert!(status.status.validation_error().is_none());
        assert_eq!(serde_json::to_string(&status).unwrap(), s);

        let full = s;
        let s = r#"{"status":"SYNCING","latestValidHash":null}"#;
        let status: PayloadStatus = serde_json::from_str(s).unwrap();
        assert_eq!(status.status, PayloadStatusEnum::Syncing);
        assert!(status.latest_valid_hash.is_none());
        assert!(status.status.validation_error().is_none());
        assert_eq!(serde_json::to_string(&status).unwrap(), full);
    }

    #[test]
    fn serde_payload_status_error_deserialize() {
        let s = r#"{"status":"INVALID","latestValidHash":null,"validationError":"Failed to decode block"}"#;
        let q = PayloadStatus {
            latest_valid_hash: None,
            status: PayloadStatusEnum::Invalid {
                validation_error: "Failed to decode block".to_string(),
            },
        };
        assert_eq!(q, serde_json::from_str(s).unwrap());

        let s = r#"{"status":"INVALID","latestValidHash":null,"validationError":"links to previously rejected block"}"#;
        let q = PayloadStatus {
            latest_valid_hash: None,
            status: PayloadStatusEnum::Invalid {
                validation_error: PayloadValidationError::LinksToRejectedPayload.to_string(),
            },
        };
        assert_eq!(q, serde_json::from_str(s).unwrap());

        let s = r#"{"status":"INVALID","latestValidHash":null,"validationError":"invalid block number"}"#;
        let q = PayloadStatus {
            latest_valid_hash: None,
            status: PayloadStatusEnum::Invalid {
                validation_error: PayloadValidationError::InvalidBlockNumber.to_string(),
            },
        };
        assert_eq!(q, serde_json::from_str(s).unwrap());

        let s = r#"{"status":"INVALID","latestValidHash":null,"validationError":
        "invalid merkle root: (remote: 0x3f77fb29ce67436532fee970e1add8f5cc80e8878c79b967af53b1fd92a0cab7 local: 0x603b9628dabdaadb442a3bb3d7e0360efc110e1948472909230909f1690fed17)"}"#;
        let q = PayloadStatus {
            latest_valid_hash: None,
            status: PayloadStatusEnum::Invalid {
                validation_error: PayloadValidationError::InvalidStateRoot {
                    remote: "0x3f77fb29ce67436532fee970e1add8f5cc80e8878c79b967af53b1fd92a0cab7"
                        .parse()
                        .unwrap(),
                    local: "0x603b9628dabdaadb442a3bb3d7e0360efc110e1948472909230909f1690fed17"
                        .parse()
                        .unwrap(),
                }
                .to_string(),
            },
        };
        similar_asserts::assert_eq!(q, serde_json::from_str(s).unwrap());
    }

    #[test]
    fn serde_roundtrip_legacy_txs_payload_v1() {
        // pulled from hive tests
        let s = r#"{"parentHash":"0x67ead97eb79b47a1638659942384143f36ed44275d4182799875ab5a87324055","feeRecipient":"0x0000000000000000000000000000000000000000","stateRoot":"0x0000000000000000000000000000000000000000000000000000000000000000","receiptsRoot":"0x4e3c608a9f2e129fccb91a1dae7472e78013b8e654bccc8d224ce3d63ae17006","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","prevRandao":"0x44bb4b98c59dbb726f96ffceb5ee028dcbe35b9bba4f9ffd56aeebf8d1e4db62","blockNumber":"0x1","gasLimit":"0x2fefd8","gasUsed":"0xa860","timestamp":"0x1235","extraData":"0x8b726574682f76302e312e30","baseFeePerGas":"0x342770c0","blockHash":"0x5655011482546f16b2312ef18e9fad03d6a52b1be95401aea884b222477f9e64","transactions":["0xf865808506fc23ac00830124f8940000000000000000000000000000000000000316018032a044b25a8b9b247d01586b3d59c71728ff49c9b84928d9e7fa3377ead3b5570b5da03ceac696601ff7ee6f5fe8864e2998db9babdf5eeba1a0cd5b4d44b3fcbd181b"]}"#;
        let payload: ExecutionPayloadV1 = serde_json::from_str(s).unwrap();
        assert_eq!(serde_json::to_string(&payload).unwrap(), s);
    }

    #[test]
    fn serde_roundtrip_legacy_txs_payload_v3() {
        // pulled from hive tests - modified with 4844 fields
        let s = r#"{"parentHash":"0x67ead97eb79b47a1638659942384143f36ed44275d4182799875ab5a87324055","feeRecipient":"0x0000000000000000000000000000000000000000","stateRoot":"0x0000000000000000000000000000000000000000000000000000000000000000","receiptsRoot":"0x4e3c608a9f2e129fccb91a1dae7472e78013b8e654bccc8d224ce3d63ae17006","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","prevRandao":"0x44bb4b98c59dbb726f96ffceb5ee028dcbe35b9bba4f9ffd56aeebf8d1e4db62","blockNumber":"0x1","gasLimit":"0x2fefd8","gasUsed":"0xa860","timestamp":"0x1235","extraData":"0x8b726574682f76302e312e30","baseFeePerGas":"0x342770c0","blockHash":"0x5655011482546f16b2312ef18e9fad03d6a52b1be95401aea884b222477f9e64","transactions":["0xf865808506fc23ac00830124f8940000000000000000000000000000000000000316018032a044b25a8b9b247d01586b3d59c71728ff49c9b84928d9e7fa3377ead3b5570b5da03ceac696601ff7ee6f5fe8864e2998db9babdf5eeba1a0cd5b4d44b3fcbd181b"],"withdrawals":[],"blobGasUsed":"0xb10b","excessBlobGas":"0xb10b"}"#;
        let payload: ExecutionPayloadV3 = serde_json::from_str(s).unwrap();
        assert_eq!(serde_json::to_string(&payload).unwrap(), s);
    }

    #[test]
    fn serde_roundtrip_enveloped_txs_payload_v1() {
        // pulled from hive tests
        let s = r#"{"parentHash":"0x67ead97eb79b47a1638659942384143f36ed44275d4182799875ab5a87324055","feeRecipient":"0x0000000000000000000000000000000000000000","stateRoot":"0x76a03cbcb7adce07fd284c61e4fa31e5e786175cefac54a29e46ec8efa28ea41","receiptsRoot":"0x4e3c608a9f2e129fccb91a1dae7472e78013b8e654bccc8d224ce3d63ae17006","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","prevRandao":"0x028111cb7d25918386a69656b3d17b2febe95fd0f11572c1a55c14f99fdfe3df","blockNumber":"0x1","gasLimit":"0x2fefd8","gasUsed":"0xa860","timestamp":"0x1235","extraData":"0x8b726574682f76302e312e30","baseFeePerGas":"0x342770c0","blockHash":"0xa6f40ed042e61e88e76125dede8fff8026751ea14454b68fb534cea99f2b2a77","transactions":["0xf865808506fc23ac00830124f8940000000000000000000000000000000000000316018032a044b25a8b9b247d01586b3d59c71728ff49c9b84928d9e7fa3377ead3b5570b5da03ceac696601ff7ee6f5fe8864e2998db9babdf5eeba1a0cd5b4d44b3fcbd181b"]}"#;
        let payload: ExecutionPayloadV1 = serde_json::from_str(s).unwrap();
        assert_eq!(serde_json::to_string(&payload).unwrap(), s);
    }

    #[test]
    fn serde_roundtrip_enveloped_txs_payload_v3() {
        // pulled from hive tests - modified with 4844 fields
        let s = r#"{"parentHash":"0x67ead97eb79b47a1638659942384143f36ed44275d4182799875ab5a87324055","feeRecipient":"0x0000000000000000000000000000000000000000","stateRoot":"0x76a03cbcb7adce07fd284c61e4fa31e5e786175cefac54a29e46ec8efa28ea41","receiptsRoot":"0x4e3c608a9f2e129fccb91a1dae7472e78013b8e654bccc8d224ce3d63ae17006","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","prevRandao":"0x028111cb7d25918386a69656b3d17b2febe95fd0f11572c1a55c14f99fdfe3df","blockNumber":"0x1","gasLimit":"0x2fefd8","gasUsed":"0xa860","timestamp":"0x1235","extraData":"0x8b726574682f76302e312e30","baseFeePerGas":"0x342770c0","blockHash":"0xa6f40ed042e61e88e76125dede8fff8026751ea14454b68fb534cea99f2b2a77","transactions":["0xf865808506fc23ac00830124f8940000000000000000000000000000000000000316018032a044b25a8b9b247d01586b3d59c71728ff49c9b84928d9e7fa3377ead3b5570b5da03ceac696601ff7ee6f5fe8864e2998db9babdf5eeba1a0cd5b4d44b3fcbd181b"],"withdrawals":[],"blobGasUsed":"0xb10b","excessBlobGas":"0xb10b"}"#;
        let payload: ExecutionPayloadV3 = serde_json::from_str(s).unwrap();
        assert_eq!(serde_json::to_string(&payload).unwrap(), s);
    }

    #[test]
    fn serde_roundtrip_execution_payload_envelope_v3() {
        // pulled from a geth response getPayloadV3 in hive tests
        let response = r#"{"executionPayload":{"parentHash":"0xe927a1448525fb5d32cb50ee1408461a945ba6c39bd5cf5621407d500ecc8de9","feeRecipient":"0x0000000000000000000000000000000000000000","stateRoot":"0x10f8a0830000e8edef6d00cc727ff833f064b1950afd591ae41357f97e543119","receiptsRoot":"0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","prevRandao":"0xe0d8b4521a7da1582a713244ffb6a86aa1726932087386e2dc7973f43fc6cb24","blockNumber":"0x1","gasLimit":"0x2ffbd2","gasUsed":"0x0","timestamp":"0x1235","extraData":"0xd883010d00846765746888676f312e32312e30856c696e7578","baseFeePerGas":"0x342770c0","blockHash":"0x44d0fa5f2f73a938ebb96a2a21679eb8dea3e7b7dd8fd9f35aa756dda8bf0a8a","transactions":[],"withdrawals":[],"blobGasUsed":"0x0","excessBlobGas":"0x0"},"blockValue":"0x0","blobsBundle":{"commitments":[],"proofs":[],"blobs":[]},"shouldOverrideBuilder":false}"#;
        let envelope: ExecutionPayloadEnvelopeV3 = serde_json::from_str(response).unwrap();
        assert_eq!(serde_json::to_string(&envelope).unwrap(), response);
    }

    #[test]
    fn serde_deserialize_execution_payload_input_v2() {
        let response = r#"
{
  "baseFeePerGas": "0x173b30b3",
  "blockHash": "0x99d486755fd046ad0bbb60457bac93d4856aa42fa00629cc7e4a28b65b5f8164",
  "blockNumber": "0xb",
  "extraData": "0xd883010d01846765746888676f312e32302e33856c696e7578",
  "feeRecipient": "0x0000000000000000000000000000000000000000",
  "gasLimit": "0x405829",
  "gasUsed": "0x3f0ca0",
  "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
  "parentHash": "0xfe34aaa2b869c66a727783ee5ad3e3983b6ef22baf24a1e502add94e7bcac67a",
  "prevRandao": "0x74132c32fe3ab9a470a8352544514d21b6969e7749f97742b53c18a1b22b396c",
  "receiptsRoot": "0x6a5c41dc55a1bd3e74e7f6accc799efb08b00c36c15265058433fcea6323e95f",
  "stateRoot": "0xde3b357f5f099e4c33d0343c9e9d204d663d7bd9c65020a38e5d0b2a9ace78a2",
  "timestamp": "0x6507d6b4",
  "transactions": [
    "0xf86d0a8458b20efd825208946177843db3138ae69679a54b95cf345ed759450d8806f3e8d87878800080820a95a0f8bddb1dcc4558b532ff747760a6f547dd275afdbe7bdecc90680e71de105757a014f34ba38c180913c0543b0ac2eccfb77cc3f801a535008dc50e533fbe435f53",
    "0xf86d0b8458b20efd82520894687704db07e902e9a8b3754031d168d46e3d586e8806f3e8d87878800080820a95a0e3108f710902be662d5c978af16109961ffaf2ac4f88522407d40949a9574276a0205719ed21889b42ab5c1026d40b759a507c12d92db0d100fa69e1ac79137caa",
    "0xf86d0c8458b20efd8252089415e6a5a2e131dd5467fa1ff3acd104f45ee5940b8806f3e8d87878800080820a96a0af556ba9cda1d686239e08c24e169dece7afa7b85e0948eaa8d457c0561277fca029da03d3af0978322e54ac7e8e654da23934e0dd839804cb0430f8aaafd732dc",
    "0xf8521784565adcb7830186a0808080820a96a0ec782872a673a9fe4eff028a5bdb30d6b8b7711f58a187bf55d3aec9757cb18ea001796d373da76f2b0aeda72183cce0ad070a4f03aa3e6fee4c757a9444245206",
    "0xf8521284565adcb7830186a0808080820a95a08a0ea89028eff02596b385a10e0bd6ae098f3b281be2c95a9feb1685065d7384a06239d48a72e4be767bd12f317dd54202f5623a33e71e25a87cb25dd781aa2fc8",
    "0xf8521384565adcb7830186a0808080820a95a0784dbd311a82f822184a46f1677a428cbe3a2b88a798fb8ad1370cdbc06429e8a07a7f6a0efd428e3d822d1de9a050b8a883938b632185c254944dd3e40180eb79"
  ],
  "withdrawals": []
}
        "#;
        let payload: ExecutionPayloadInputV2 = serde_json::from_str(response).unwrap();
        assert_eq!(payload.withdrawals, Some(vec![]));

        let response = r#"
{
  "baseFeePerGas": "0x173b30b3",
  "blockHash": "0x99d486755fd046ad0bbb60457bac93d4856aa42fa00629cc7e4a28b65b5f8164",
  "blockNumber": "0xb",
  "extraData": "0xd883010d01846765746888676f312e32302e33856c696e7578",
  "feeRecipient": "0x0000000000000000000000000000000000000000",
  "gasLimit": "0x405829",
  "gasUsed": "0x3f0ca0",
  "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
  "parentHash": "0xfe34aaa2b869c66a727783ee5ad3e3983b6ef22baf24a1e502add94e7bcac67a",
  "prevRandao": "0x74132c32fe3ab9a470a8352544514d21b6969e7749f97742b53c18a1b22b396c",
  "receiptsRoot": "0x6a5c41dc55a1bd3e74e7f6accc799efb08b00c36c15265058433fcea6323e95f",
  "stateRoot": "0xde3b357f5f099e4c33d0343c9e9d204d663d7bd9c65020a38e5d0b2a9ace78a2",
  "timestamp": "0x6507d6b4",
  "transactions": [
    "0xf86d0a8458b20efd825208946177843db3138ae69679a54b95cf345ed759450d8806f3e8d87878800080820a95a0f8bddb1dcc4558b532ff747760a6f547dd275afdbe7bdecc90680e71de105757a014f34ba38c180913c0543b0ac2eccfb77cc3f801a535008dc50e533fbe435f53",
    "0xf86d0b8458b20efd82520894687704db07e902e9a8b3754031d168d46e3d586e8806f3e8d87878800080820a95a0e3108f710902be662d5c978af16109961ffaf2ac4f88522407d40949a9574276a0205719ed21889b42ab5c1026d40b759a507c12d92db0d100fa69e1ac79137caa",
    "0xf86d0c8458b20efd8252089415e6a5a2e131dd5467fa1ff3acd104f45ee5940b8806f3e8d87878800080820a96a0af556ba9cda1d686239e08c24e169dece7afa7b85e0948eaa8d457c0561277fca029da03d3af0978322e54ac7e8e654da23934e0dd839804cb0430f8aaafd732dc",
    "0xf8521784565adcb7830186a0808080820a96a0ec782872a673a9fe4eff028a5bdb30d6b8b7711f58a187bf55d3aec9757cb18ea001796d373da76f2b0aeda72183cce0ad070a4f03aa3e6fee4c757a9444245206",
    "0xf8521284565adcb7830186a0808080820a95a08a0ea89028eff02596b385a10e0bd6ae098f3b281be2c95a9feb1685065d7384a06239d48a72e4be767bd12f317dd54202f5623a33e71e25a87cb25dd781aa2fc8",
    "0xf8521384565adcb7830186a0808080820a95a0784dbd311a82f822184a46f1677a428cbe3a2b88a798fb8ad1370cdbc06429e8a07a7f6a0efd428e3d822d1de9a050b8a883938b632185c254944dd3e40180eb79"
  ]
}
        "#;
        let payload: ExecutionPayloadInputV2 = serde_json::from_str(response).unwrap();
        assert_eq!(payload.withdrawals, None);
    }

    #[test]
    fn serde_deserialize_v3_with_unknown_fields() {
        let input = r#"
{
    "parentHash": "0xaaa4c5b574f37e1537c78931d1bca24a4d17d4f29f1ee97e1cd48b704909de1f",
    "feeRecipient": "0x2adc25665018aa1fe0e6bc666dac8fc2697ff9ba",
    "stateRoot": "0x308ee9c5c6fab5e3d08763a3b5fe0be8ada891fa5010a49a3390e018dd436810",
    "receiptsRoot": "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
    "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
    "prevRandao": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "blockNumber": "0xf",
    "gasLimit": "0x16345785d8a0000",
    "gasUsed": "0x0",
    "timestamp": "0x3a97",
    "extraData": "0x",
    "baseFeePerGas": "0x7",
    "blockHash": "0x38bb6ba645c7e6bd970f9c7d492fafe1e04d85349054cb48d16c9d2c3e3cd0bf",
    "transactions": [],
    "withdrawals": [],
    "excessBlobGas": "0x0",
    "blobGasUsed": "0x0"
}
        "#;

        // ensure that deserializing this succeeds
        let _payload_res: ExecutionPayloadV3 = serde_json::from_str(input).unwrap();

        // construct a payload with a random field in the middle
        let input = r#"
{
    "parentHash": "0xaaa4c5b574f37e1537c78931d1bca24a4d17d4f29f1ee97e1cd48b704909de1f",
    "feeRecipient": "0x2adc25665018aa1fe0e6bc666dac8fc2697ff9ba",
    "stateRoot": "0x308ee9c5c6fab5e3d08763a3b5fe0be8ada891fa5010a49a3390e018dd436810",
    "receiptsRoot": "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
    "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
    "prevRandao": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "blockNumber": "0xf",
    "gasLimit": "0x16345785d8a0000",
    "gasUsed": "0x0",
    "timestamp": "0x3a97",
    "extraData": "0x",
    "baseFeePerGas": "0x7",
    "blockHash": "0x38bb6ba645c7e6bd970f9c7d492fafe1e04d85349054cb48d16c9d2c3e3cd0bf",
    "transactions": [],
    "withdrawals": [],
    "randomStuff": [],
    "excessBlobGas": "0x0",
    "blobGasUsed": "0x0"
}
        "#;

        // ensure that deserializing this fails
        let _payload_res = serde_json::from_str::<ExecutionPayloadV3>(input).unwrap_err();

        // construct a payload with a random field at the end
        let input = r#"
{
    "parentHash": "0xaaa4c5b574f37e1537c78931d1bca24a4d17d4f29f1ee97e1cd48b704909de1f",
    "feeRecipient": "0x2adc25665018aa1fe0e6bc666dac8fc2697ff9ba",
    "stateRoot": "0x308ee9c5c6fab5e3d08763a3b5fe0be8ada891fa5010a49a3390e018dd436810",
    "receiptsRoot": "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
    "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
    "prevRandao": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "blockNumber": "0xf",
    "gasLimit": "0x16345785d8a0000",
    "gasUsed": "0x0",
    "timestamp": "0x3a97",
    "extraData": "0x",
    "baseFeePerGas": "0x7",
    "blockHash": "0x38bb6ba645c7e6bd970f9c7d492fafe1e04d85349054cb48d16c9d2c3e3cd0bf",
    "transactions": [],
    "withdrawals": [],
    "randomStuff": [],
    "excessBlobGas": "0x0",
    "blobGasUsed": "0x0"
    "moreRandomStuff": "0x0",
}
        "#;

        // ensure that deserializing this fails
        let _payload_res = serde_json::from_str::<ExecutionPayloadV3>(input).unwrap_err();
    }

    #[test]
    fn serde_deserialize_v2_input_with_blob_fields() {
        let input = r#"
{
    "parentHash": "0xaaa4c5b574f37e1537c78931d1bca24a4d17d4f29f1ee97e1cd48b704909de1f",
    "feeRecipient": "0x2adc25665018aa1fe0e6bc666dac8fc2697ff9ba",
    "stateRoot": "0x308ee9c5c6fab5e3d08763a3b5fe0be8ada891fa5010a49a3390e018dd436810",
    "receiptsRoot": "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
    "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
    "prevRandao": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "blockNumber": "0xf",
    "gasLimit": "0x16345785d8a0000",
    "gasUsed": "0x0",
    "timestamp": "0x3a97",
    "extraData": "0x",
    "baseFeePerGas": "0x7",
    "blockHash": "0x38bb6ba645c7e6bd970f9c7d492fafe1e04d85349054cb48d16c9d2c3e3cd0bf",
    "transactions": [],
    "withdrawals": [],
    "excessBlobGas": "0x0",
    "blobGasUsed": "0x0"
}
        "#;

        // ensure that deserializing this (it includes blob fields) fails
        let payload_res: Result<ExecutionPayloadInputV2, serde_json::Error> =
            serde_json::from_str(input);
        assert!(payload_res.is_err());
    }
}
