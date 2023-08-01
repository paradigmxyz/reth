use reth_primitives::{
    constants::{MAXIMUM_EXTRA_DATA_SIZE, MIN_PROTOCOL_BASE_FEE_U256},
    proofs::{self, EMPTY_LIST_HASH},
    Address, Block, Bloom, Bytes, Header, SealedBlock, TransactionSigned, UintTryTo, Withdrawal,
    H256, H64, U256, U64,
};
use reth_rlp::Decodable;
use serde::{ser::SerializeMap, Deserialize, Serialize, Serializer};

/// The execution payload body response that allows for `null` values.
pub type ExecutionPayloadBodiesV1 = Vec<Option<ExecutionPayloadBodyV1>>;

/// And 8-byte identifier for an execution payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct PayloadId(H64);

// === impl PayloadId ===

impl PayloadId {
    /// Creates a new payload id from the given identifier.
    pub fn new(id: [u8; 8]) -> Self {
        Self(H64::from(id))
    }
}

impl std::fmt::Display for PayloadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// This structure maps for the return value of `engine_getPayloadV2` of the beacon chain spec.
///
/// See also: <https://github.com/ethereum/execution-apis/blob/main/src/engine/shanghai.md#engine_getpayloadv2>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPayloadEnvelope {
    /// Execution payload, which could be either V1 or V2
    ///
    /// V1 (_NO_ withdrawals) MUST be returned if the payload timestamp is lower than the Shanghai
    /// timestamp
    ///
    /// V2 (_WITH_ withdrawals) MUST be returned if the payload timestamp is greater or equal to
    /// the Shanghai timestamp
    #[serde(rename = "executionPayload")]
    pub payload: ExecutionPayload,
    /// The expected value to be received by the feeRecipient in wei
    #[serde(rename = "blockValue")]
    pub block_value: U256,
    //
    // // TODO(mattsse): for V3
    // #[serde(rename = "blobsBundle", skip_serializing_if = "Option::is_none")]
    // pub blobs_bundle: Option<BlobsBundleV1>,
}

impl ExecutionPayloadEnvelope {
    /// Returns the [ExecutionPayload] for the `engine_getPayloadV1` endpoint
    pub fn into_v1_payload(mut self) -> ExecutionPayload {
        // ensure withdrawals are removed
        self.payload.withdrawals.take();
        self.payload
    }
}

/// This structure maps on the ExecutionPayload structure of the beacon chain spec.
///
/// See also: <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#executionpayloadv1>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPayload {
    pub parent_hash: H256,
    pub fee_recipient: Address,
    pub state_root: H256,
    pub receipts_root: H256,
    pub logs_bloom: Bloom,
    pub prev_randao: H256,
    pub block_number: U64,
    pub gas_limit: U64,
    pub gas_used: U64,
    pub timestamp: U64,
    pub extra_data: Bytes,
    pub base_fee_per_gas: U256,
    pub blob_gas_used: Option<U64>,
    pub excess_blob_gas: Option<U64>,
    pub block_hash: H256,
    pub transactions: Vec<Bytes>,
    /// Array of [`Withdrawal`] enabled with V2
    /// See <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/shanghai.md#executionpayloadv2>
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawals: Option<Vec<Withdrawal>>,
}

impl From<SealedBlock> for ExecutionPayload {
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
        ExecutionPayload {
            parent_hash: value.parent_hash,
            fee_recipient: value.beneficiary,
            state_root: value.state_root,
            receipts_root: value.receipts_root,
            logs_bloom: value.logs_bloom,
            prev_randao: value.mix_hash,
            block_number: value.number.into(),
            gas_limit: value.gas_limit.into(),
            gas_used: value.gas_used.into(),
            timestamp: value.timestamp.into(),
            extra_data: value.extra_data.clone(),
            base_fee_per_gas: U256::from(value.base_fee_per_gas.unwrap_or_default()),
            blob_gas_used: value.blob_gas_used.map(U64::from),
            excess_blob_gas: value.excess_blob_gas.map(U64::from),
            block_hash: value.hash(),
            transactions,
            withdrawals: value.withdrawals,
        }
    }
}

/// Try to construct a block from given payload. Perform addition validation of `extra_data` and
/// `base_fee_per_gas` fields.
///
/// NOTE: The log bloom is assumed to be validated during serialization.
/// NOTE: Empty ommers, nonce and difficulty values are validated upon computing block hash and
/// comparing the value with `payload.block_hash`.
///
/// See <https://github.com/ethereum/go-ethereum/blob/79a478bb6176425c2400e949890e668a3d9a3d05/core/beacon/types.go#L145>
impl TryFrom<ExecutionPayload> for SealedBlock {
    type Error = PayloadError;

    fn try_from(payload: ExecutionPayload) -> Result<Self, Self::Error> {
        if payload.extra_data.len() > MAXIMUM_EXTRA_DATA_SIZE {
            return Err(PayloadError::ExtraData(payload.extra_data))
        }

        if payload.base_fee_per_gas < MIN_PROTOCOL_BASE_FEE_U256 {
            return Err(PayloadError::BaseFee(payload.base_fee_per_gas))
        }

        let transactions = payload
            .transactions
            .iter()
            .map(|tx| TransactionSigned::decode(&mut tx.as_ref()))
            .collect::<Result<Vec<_>, _>>()?;
        let transactions_root = proofs::calculate_transaction_root(&transactions);

        let withdrawals_root =
            payload.withdrawals.as_ref().map(|w| proofs::calculate_withdrawals_root(w));

        let header = Header {
            parent_hash: payload.parent_hash,
            beneficiary: payload.fee_recipient,
            state_root: payload.state_root,
            transactions_root,
            receipts_root: payload.receipts_root,
            withdrawals_root,
            logs_bloom: payload.logs_bloom,
            number: payload.block_number.as_u64(),
            gas_limit: payload.gas_limit.as_u64(),
            gas_used: payload.gas_used.as_u64(),
            timestamp: payload.timestamp.as_u64(),
            mix_hash: payload.prev_randao,
            base_fee_per_gas: Some(
                payload
                    .base_fee_per_gas
                    .uint_try_to()
                    .map_err(|_| PayloadError::BaseFee(payload.base_fee_per_gas))?,
            ),
            blob_gas_used: payload.blob_gas_used.map(|blob_gas_used| blob_gas_used.as_u64()),
            excess_blob_gas: payload
                .excess_blob_gas
                .map(|excess_blob_gas| excess_blob_gas.as_u64()),
            extra_data: payload.extra_data,
            // Defaults
            ommers_hash: EMPTY_LIST_HASH,
            difficulty: Default::default(),
            nonce: Default::default(),
        }
        .seal_slow();

        if payload.block_hash != header.hash() {
            return Err(PayloadError::BlockHash {
                execution: header.hash(),
                consensus: payload.block_hash,
            })
        }

        Ok(SealedBlock {
            header,
            body: transactions,
            withdrawals: payload.withdrawals,
            ommers: Default::default(),
        })
    }
}

/// This includes all bundled blob related data of an executed payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobsBundleV1 {
    pub commitments: Vec<Bytes>,
    pub proofs: Vec<Bytes>,
    pub blobs: Vec<Bytes>,
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
    /// Invalid payload base fee.
    #[error("Invalid payload blob gas used: {0}")]
    BlobGasUsed(U256),
    /// Invalid payload base fee.
    #[error("Invalid payload excess blob gas: {0}")]
    ExcessBlobGas(U256),
    /// Invalid payload block hash.
    #[error("blockhash mismatch, want {consensus}, got {execution}")]
    BlockHash {
        /// The block hash computed from the payload.
        execution: H256,
        /// The block hash provided with the payload.
        consensus: H256,
    },
    /// Encountered decoding error.
    #[error(transparent)]
    Decode(#[from] reth_rlp::DecodeError),
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

impl From<Block> for ExecutionPayloadBodyV1 {
    fn from(value: Block) -> Self {
        let transactions = value.body.into_iter().map(|tx| {
            let mut out = Vec::new();
            tx.encode_enveloped(&mut out);
            out.into()
        });
        ExecutionPayloadBodyV1 {
            transactions: transactions.collect(),
            withdrawals: value.withdrawals,
        }
    }
}

/// This structure contains the attributes required to initiate a payload build process in the
/// context of an `engine_forkchoiceUpdated` call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadAttributes {
    pub timestamp: U64,
    pub prev_randao: H256,
    pub suggested_fee_recipient: Address,
    /// Array of [`Withdrawal`] enabled with V2
    /// See <https://github.com/ethereum/execution-apis/blob/6452a6b194d7db269bf1dbd087a267251d3cc7f8/src/engine/shanghai.md#payloadattributesv2>
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawals: Option<Vec<Withdrawal>>,
    /// Root of the parent beacon block enabled with V3.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/cancun.md#payloadattributesv3>
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_beacon_block_root: Option<H256>,
}

/// This structure contains the result of processing a payload or fork choice update.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadStatus {
    #[serde(flatten)]
    pub status: PayloadStatusEnum,
    /// Hash of the most recent valid block in the branch defined by payload and its ancestors
    pub latest_valid_hash: Option<H256>,
}

impl PayloadStatus {
    pub fn new(status: PayloadStatusEnum, latest_valid_hash: Option<H256>) -> Self {
        Self { status, latest_valid_hash }
    }

    pub fn from_status(status: PayloadStatusEnum) -> Self {
        Self { status, latest_valid_hash: None }
    }

    pub fn with_latest_valid_hash(mut self, latest_valid_hash: H256) -> Self {
        self.latest_valid_hash = Some(latest_valid_hash);
        self
    }

    pub fn maybe_latest_valid_hash(mut self, latest_valid_hash: Option<H256>) -> Self {
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
        remote: H256,
        /// The state root of the payload that we computed locally.
        local: H256,
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
    fn serde_roundtrip_legacy_txs_payload() {
        // pulled from hive tests
        let s = r#"{"parentHash":"0x67ead97eb79b47a1638659942384143f36ed44275d4182799875ab5a87324055","feeRecipient":"0x0000000000000000000000000000000000000000","stateRoot":"0x0000000000000000000000000000000000000000000000000000000000000000","receiptsRoot":"0x4e3c608a9f2e129fccb91a1dae7472e78013b8e654bccc8d224ce3d63ae17006","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","prevRandao":"0x44bb4b98c59dbb726f96ffceb5ee028dcbe35b9bba4f9ffd56aeebf8d1e4db62","blockNumber":"0x1","gasLimit":"0x2fefd8","gasUsed":"0xa860","timestamp":"0x1235","extraData":"0x8b726574682f76302e312e30","baseFeePerGas":"0x342770c0","blobGasUsed":null,"excessBlobGas":null,"blockHash":"0x5655011482546f16b2312ef18e9fad03d6a52b1be95401aea884b222477f9e64","transactions":["0xf865808506fc23ac00830124f8940000000000000000000000000000000000000316018032a044b25a8b9b247d01586b3d59c71728ff49c9b84928d9e7fa3377ead3b5570b5da03ceac696601ff7ee6f5fe8864e2998db9babdf5eeba1a0cd5b4d44b3fcbd181b"]}"#;
        let payload: ExecutionPayload = serde_json::from_str(s).unwrap();
        assert_eq!(serde_json::to_string(&payload).unwrap(), s);
    }

    #[test]
    fn serde_roundtrip_enveloped_txs_payload() {
        // pulled from hive tests
        let s = r#"{"parentHash":"0x67ead97eb79b47a1638659942384143f36ed44275d4182799875ab5a87324055","feeRecipient":"0x0000000000000000000000000000000000000000","stateRoot":"0x76a03cbcb7adce07fd284c61e4fa31e5e786175cefac54a29e46ec8efa28ea41","receiptsRoot":"0x4e3c608a9f2e129fccb91a1dae7472e78013b8e654bccc8d224ce3d63ae17006","logsBloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","prevRandao":"0x028111cb7d25918386a69656b3d17b2febe95fd0f11572c1a55c14f99fdfe3df","blockNumber":"0x1","gasLimit":"0x2fefd8","gasUsed":"0xa860","timestamp":"0x1235","extraData":"0x8b726574682f76302e312e30","baseFeePerGas":"0x342770c0","blobGasUsed":null,"excessBlobGas":null,"blockHash":"0xa6f40ed042e61e88e76125dede8fff8026751ea14454b68fb534cea99f2b2a77","transactions":["0xf865808506fc23ac00830124f8940000000000000000000000000000000000000316018032a044b25a8b9b247d01586b3d59c71728ff49c9b84928d9e7fa3377ead3b5570b5da03ceac696601ff7ee6f5fe8864e2998db9babdf5eeba1a0cd5b4d44b3fcbd181b"]}"#;
        let payload: ExecutionPayload = serde_json::from_str(s).unwrap();
        assert_eq!(serde_json::to_string(&payload).unwrap(), s);
    }
}
