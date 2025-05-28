//! Contains types required for building a payload.

use alloc::{sync::Arc, vec::Vec};
use alloy_eips::{
    eip4844::BlobTransactionSidecar,
    eip4895::Withdrawals,
    eip7594::{BlobTransactionSidecarEip7594, BlobTransactionSidecarVariant},
    eip7685::Requests,
};
use alloy_primitives::{Address, B256, U256};
use alloy_rlp::Encodable;
use alloy_rpc_types_engine::{
    BlobsBundleV1, BlobsBundleV2, ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3,
    ExecutionPayloadEnvelopeV4, ExecutionPayloadEnvelopeV5, ExecutionPayloadFieldV2,
    ExecutionPayloadV1, ExecutionPayloadV3, PayloadAttributes, PayloadId,
};
use core::convert::Infallible;
use reth_ethereum_primitives::{Block, EthPrimitives};
use reth_payload_primitives::{BuiltPayload, PayloadBuilderAttributes};
use reth_primitives_traits::SealedBlock;

use crate::BuiltPayloadConversionError;

/// Contains the built payload.
///
/// According to the [engine API specification](https://github.com/ethereum/execution-apis/blob/main/src/engine/README.md) the execution layer should build the initial version of the payload with an empty transaction set and then keep update it in order to maximize the revenue.
/// Therefore, the empty-block here is always available and full-block will be set/updated
/// afterward.
#[derive(Debug, Clone)]
pub struct EthBuiltPayload {
    /// Identifier of the payload
    pub(crate) id: PayloadId,
    /// The built block
    pub(crate) block: Arc<SealedBlock<Block>>,
    /// The fees of the block
    pub(crate) fees: U256,
    /// The blobs, proofs, and commitments in the block. If the block is pre-cancun, this will be
    /// empty.
    pub(crate) sidecars: BlobSidecars,
    /// The requests of the payload
    pub(crate) requests: Option<Requests>,
}

// === impl BuiltPayload ===

impl EthBuiltPayload {
    /// Initializes the payload with the given initial block
    ///
    /// Caution: This does not set any [`BlobSidecars`].
    pub const fn new(
        id: PayloadId,
        block: Arc<SealedBlock<Block>>,
        fees: U256,
        requests: Option<Requests>,
    ) -> Self {
        Self { id, block, fees, requests, sidecars: BlobSidecars::Empty }
    }

    /// Returns the identifier of the payload.
    pub const fn id(&self) -> PayloadId {
        self.id
    }

    /// Returns the built block(sealed)
    pub fn block(&self) -> &SealedBlock<Block> {
        &self.block
    }

    /// Fees of the block
    pub const fn fees(&self) -> U256 {
        self.fees
    }

    /// Returns the blob sidecars.
    pub const fn sidecars(&self) -> &BlobSidecars {
        &self.sidecars
    }

    /// Sets blob transactions sidecars on the payload.
    pub fn with_sidecars(mut self, sidecars: impl Into<BlobSidecars>) -> Self {
        self.sidecars = sidecars.into();
        self
    }

    /// Try converting built payload into [`ExecutionPayloadEnvelopeV3`].
    ///
    /// Returns an error if the payload contains non EIP-4844 sidecar.
    pub fn try_into_v3(self) -> Result<ExecutionPayloadEnvelopeV3, BuiltPayloadConversionError> {
        let Self { block, fees, sidecars, .. } = self;

        let blobs_bundle = match sidecars {
            BlobSidecars::Empty => BlobsBundleV1::empty(),
            BlobSidecars::Eip4844(sidecars) => BlobsBundleV1::from(sidecars),
            BlobSidecars::Eip7594(_) => {
                return Err(BuiltPayloadConversionError::UnexpectedEip7594Sidecars)
            }
        };

        Ok(ExecutionPayloadEnvelopeV3 {
            execution_payload: ExecutionPayloadV3::from_block_unchecked(
                block.hash(),
                &Arc::unwrap_or_clone(block).into_block(),
            ),
            block_value: fees,
            // From the engine API spec:
            //
            // > Client software **MAY** use any heuristics to decide whether to set
            // `shouldOverrideBuilder` flag or not. If client software does not implement any
            // heuristic this flag **SHOULD** be set to `false`.
            //
            // Spec:
            // <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#specification-2>
            should_override_builder: false,
            blobs_bundle,
        })
    }

    /// Try converting built payload into [`ExecutionPayloadEnvelopeV4`].
    ///
    /// Returns an error if the payload contains non EIP-4844 sidecar.
    pub fn try_into_v4(self) -> Result<ExecutionPayloadEnvelopeV4, BuiltPayloadConversionError> {
        Ok(ExecutionPayloadEnvelopeV4 {
            execution_requests: self.requests.clone().unwrap_or_default(),
            envelope_inner: self.try_into()?,
        })
    }

    /// Try converting built payload into [`ExecutionPayloadEnvelopeV5`].
    pub fn try_into_v5(self) -> Result<ExecutionPayloadEnvelopeV5, BuiltPayloadConversionError> {
        let Self { block, fees, sidecars, requests, .. } = self;

        let blobs_bundle = match sidecars {
            BlobSidecars::Empty => BlobsBundleV2::empty(),
            BlobSidecars::Eip7594(sidecars) => BlobsBundleV2::from(sidecars),
            BlobSidecars::Eip4844(_) => {
                return Err(BuiltPayloadConversionError::UnexpectedEip4844Sidecars)
            }
        };

        Ok(ExecutionPayloadEnvelopeV5 {
            execution_payload: ExecutionPayloadV3::from_block_unchecked(
                block.hash(),
                &Arc::unwrap_or_clone(block).into_block(),
            ),
            block_value: fees,
            // From the engine API spec:
            //
            // > Client software **MAY** use any heuristics to decide whether to set
            // `shouldOverrideBuilder` flag or not. If client software does not implement any
            // heuristic this flag **SHOULD** be set to `false`.
            //
            // Spec:
            // <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#specification-2>
            should_override_builder: false,
            blobs_bundle,
            execution_requests: requests.unwrap_or_default(),
        })
    }
}

impl BuiltPayload for EthBuiltPayload {
    type Primitives = EthPrimitives;

    fn block(&self) -> &SealedBlock<Block> {
        &self.block
    }

    fn fees(&self) -> U256 {
        self.fees
    }

    fn requests(&self) -> Option<Requests> {
        self.requests.clone()
    }
}

// V1 engine_getPayloadV1 response
impl From<EthBuiltPayload> for ExecutionPayloadV1 {
    fn from(value: EthBuiltPayload) -> Self {
        Self::from_block_unchecked(
            value.block().hash(),
            &Arc::unwrap_or_clone(value.block).into_block(),
        )
    }
}

// V2 engine_getPayloadV2 response
impl From<EthBuiltPayload> for ExecutionPayloadEnvelopeV2 {
    fn from(value: EthBuiltPayload) -> Self {
        let EthBuiltPayload { block, fees, .. } = value;

        Self {
            block_value: fees,
            execution_payload: ExecutionPayloadFieldV2::from_block_unchecked(
                block.hash(),
                &Arc::unwrap_or_clone(block).into_block(),
            ),
        }
    }
}

impl TryFrom<EthBuiltPayload> for ExecutionPayloadEnvelopeV3 {
    type Error = BuiltPayloadConversionError;

    fn try_from(value: EthBuiltPayload) -> Result<Self, Self::Error> {
        value.try_into_v3()
    }
}

impl TryFrom<EthBuiltPayload> for ExecutionPayloadEnvelopeV4 {
    type Error = BuiltPayloadConversionError;

    fn try_from(value: EthBuiltPayload) -> Result<Self, Self::Error> {
        value.try_into_v4()
    }
}

impl TryFrom<EthBuiltPayload> for ExecutionPayloadEnvelopeV5 {
    type Error = BuiltPayloadConversionError;

    fn try_from(value: EthBuiltPayload) -> Result<Self, Self::Error> {
        value.try_into_v5()
    }
}

/// An enum representing blob transaction sidecars belonging to [`EthBuiltPayload`].
#[derive(Clone, Default, Debug)]
pub enum BlobSidecars {
    /// No sidecars (default).
    #[default]
    Empty,
    /// EIP-4844 style sidecars.
    Eip4844(Vec<BlobTransactionSidecar>),
    /// EIP-7594 style sidecars.
    Eip7594(Vec<BlobTransactionSidecarEip7594>),
}

impl BlobSidecars {
    /// Create new EIP-4844 style sidecars.
    pub const fn eip4844(sidecars: Vec<BlobTransactionSidecar>) -> Self {
        Self::Eip4844(sidecars)
    }

    /// Create new EIP-7594 style sidecars.
    pub const fn eip7594(sidecars: Vec<BlobTransactionSidecarEip7594>) -> Self {
        Self::Eip7594(sidecars)
    }

    /// Push EIP-4844 blob sidecar. Ignores the item if sidecars already contain EIP-7594 sidecars.
    pub fn push_eip4844_sidecar(&mut self, sidecar: BlobTransactionSidecar) {
        match self {
            Self::Empty => {
                *self = Self::Eip4844(Vec::from([sidecar]));
            }
            Self::Eip4844(sidecars) => {
                sidecars.push(sidecar);
            }
            Self::Eip7594(_) => {}
        }
    }

    /// Push EIP-7594 blob sidecar. Ignores the item if sidecars already contain EIP-4844 sidecars.
    pub fn push_eip7594_sidecar(&mut self, sidecar: BlobTransactionSidecarEip7594) {
        match self {
            Self::Empty => {
                *self = Self::Eip7594(Vec::from([sidecar]));
            }
            Self::Eip7594(sidecars) => {
                sidecars.push(sidecar);
            }
            Self::Eip4844(_) => {}
        }
    }

    /// Push a [`BlobTransactionSidecarVariant`]. Ignores the item if sidecars already contain the
    /// opposite type.
    pub fn push_sidecar_variant(&mut self, sidecar: BlobTransactionSidecarVariant) {
        match sidecar {
            BlobTransactionSidecarVariant::Eip4844(sidecar) => {
                self.push_eip4844_sidecar(sidecar);
            }
            BlobTransactionSidecarVariant::Eip7594(sidecar) => {
                self.push_eip7594_sidecar(sidecar);
            }
        }
    }
}

impl From<Vec<BlobTransactionSidecar>> for BlobSidecars {
    fn from(value: Vec<BlobTransactionSidecar>) -> Self {
        Self::eip4844(value)
    }
}

impl From<Vec<BlobTransactionSidecarEip7594>> for BlobSidecars {
    fn from(value: Vec<BlobTransactionSidecarEip7594>) -> Self {
        Self::eip7594(value)
    }
}

impl From<alloc::vec::IntoIter<BlobTransactionSidecar>> for BlobSidecars {
    fn from(value: alloc::vec::IntoIter<BlobTransactionSidecar>) -> Self {
        value.collect::<Vec<_>>().into()
    }
}

impl From<alloc::vec::IntoIter<BlobTransactionSidecarEip7594>> for BlobSidecars {
    fn from(value: alloc::vec::IntoIter<BlobTransactionSidecarEip7594>) -> Self {
        value.collect::<Vec<_>>().into()
    }
}

/// Container type for all components required to build a payload.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EthPayloadBuilderAttributes {
    /// Id of the payload
    pub id: PayloadId,
    /// Parent block to build the payload on top
    pub parent: B256,
    /// Unix timestamp for the generated payload
    ///
    /// Number of seconds since the Unix epoch.
    pub timestamp: u64,
    /// Address of the recipient for collecting transaction fee
    pub suggested_fee_recipient: Address,
    /// Randomness value for the generated payload
    pub prev_randao: B256,
    /// Withdrawals for the generated payload
    pub withdrawals: Withdrawals,
    /// Root of the parent beacon block
    pub parent_beacon_block_root: Option<B256>,
}

// === impl EthPayloadBuilderAttributes ===

impl EthPayloadBuilderAttributes {
    /// Returns the identifier of the payload.
    pub const fn payload_id(&self) -> PayloadId {
        self.id
    }

    /// Creates a new payload builder for the given parent block and the attributes.
    ///
    /// Derives the unique [`PayloadId`] for the given parent and attributes
    pub fn new(parent: B256, attributes: PayloadAttributes) -> Self {
        let id = payload_id(&parent, &attributes);

        Self {
            id,
            parent,
            timestamp: attributes.timestamp,
            suggested_fee_recipient: attributes.suggested_fee_recipient,
            prev_randao: attributes.prev_randao,
            withdrawals: attributes.withdrawals.unwrap_or_default().into(),
            parent_beacon_block_root: attributes.parent_beacon_block_root,
        }
    }
}

impl PayloadBuilderAttributes for EthPayloadBuilderAttributes {
    type RpcPayloadAttributes = PayloadAttributes;
    type Error = Infallible;

    /// Creates a new payload builder for the given parent block and the attributes.
    ///
    /// Derives the unique [`PayloadId`] for the given parent and attributes
    fn try_new(
        parent: B256,
        attributes: PayloadAttributes,
        _version: u8,
    ) -> Result<Self, Infallible> {
        Ok(Self::new(parent, attributes))
    }

    fn payload_id(&self) -> PayloadId {
        self.id
    }

    fn parent(&self) -> B256 {
        self.parent
    }

    fn timestamp(&self) -> u64 {
        self.timestamp
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.parent_beacon_block_root
    }

    fn suggested_fee_recipient(&self) -> Address {
        self.suggested_fee_recipient
    }

    fn prev_randao(&self) -> B256 {
        self.prev_randao
    }

    fn withdrawals(&self) -> &Withdrawals {
        &self.withdrawals
    }
}

/// Generates the payload id for the configured payload from the [`PayloadAttributes`].
///
/// Returns an 8-byte identifier by hashing the payload components with sha256 hash.
pub(crate) fn payload_id(parent: &B256, attributes: &PayloadAttributes) -> PayloadId {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(parent.as_slice());
    hasher.update(&attributes.timestamp.to_be_bytes()[..]);
    hasher.update(attributes.prev_randao.as_slice());
    hasher.update(attributes.suggested_fee_recipient.as_slice());
    if let Some(withdrawals) = &attributes.withdrawals {
        let mut buf = Vec::new();
        withdrawals.encode(&mut buf);
        hasher.update(buf);
    }

    if let Some(parent_beacon_block) = attributes.parent_beacon_block_root {
        hasher.update(parent_beacon_block);
    }

    let out = hasher.finalize();
    PayloadId::new(out.as_slice()[..8].try_into().expect("sufficient length"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip4895::Withdrawal;
    use alloy_primitives::B64;
    use core::str::FromStr;

    #[test]
    fn attributes_serde() {
        let attributes = r#"{"timestamp":"0x1235","prevRandao":"0xf343b00e02dc34ec0124241f74f32191be28fb370bb48060f5fa4df99bda774c","suggestedFeeRecipient":"0x0000000000000000000000000000000000000000","withdrawals":null,"parentBeaconBlockRoot":null}"#;
        let _attributes: PayloadAttributes = serde_json::from_str(attributes).unwrap();
    }

    #[test]
    fn test_payload_id_basic() {
        // Create a parent block and payload attributes
        let parent =
            B256::from_str("0x3b8fb240d288781d4aac94d3fd16809ee413bc99294a085798a589dae51ddd4a")
                .unwrap();
        let attributes = PayloadAttributes {
            timestamp: 0x5,
            prev_randao: B256::from_str(
                "0x0000000000000000000000000000000000000000000000000000000000000000",
            )
            .unwrap(),
            suggested_fee_recipient: Address::from_str(
                "0xa94f5374fce5edbc8e2a8697c15331677e6ebf0b",
            )
            .unwrap(),
            withdrawals: None,
            parent_beacon_block_root: None,
        };

        // Verify that the generated payload ID matches the expected value
        assert_eq!(
            payload_id(&parent, &attributes),
            PayloadId(B64::from_str("0xa247243752eb10b4").unwrap())
        );
    }

    #[test]
    fn test_payload_id_with_withdrawals() {
        // Set up the parent and attributes with withdrawals
        let parent =
            B256::from_str("0x9876543210abcdef9876543210abcdef9876543210abcdef9876543210abcdef")
                .unwrap();
        let attributes = PayloadAttributes {
            timestamp: 1622553200,
            prev_randao: B256::from_slice(&[1; 32]),
            suggested_fee_recipient: Address::from_str(
                "0xb94f5374fce5edbc8e2a8697c15331677e6ebf0b",
            )
            .unwrap(),
            withdrawals: Some(vec![
                Withdrawal {
                    index: 1,
                    validator_index: 123,
                    address: Address::from([0xAA; 20]),
                    amount: 10,
                },
                Withdrawal {
                    index: 2,
                    validator_index: 456,
                    address: Address::from([0xBB; 20]),
                    amount: 20,
                },
            ]),
            parent_beacon_block_root: None,
        };

        // Verify that the generated payload ID matches the expected value
        assert_eq!(
            payload_id(&parent, &attributes),
            PayloadId(B64::from_str("0xedddc2f84ba59865").unwrap())
        );
    }

    #[test]
    fn test_payload_id_with_parent_beacon_block_root() {
        // Set up the parent and attributes with a parent beacon block root
        let parent =
            B256::from_str("0x9876543210abcdef9876543210abcdef9876543210abcdef9876543210abcdef")
                .unwrap();
        let attributes = PayloadAttributes {
            timestamp: 1622553200,
            prev_randao: B256::from_str(
                "0x123456789abcdef123456789abcdef123456789abcdef123456789abcdef1234",
            )
            .unwrap(),
            suggested_fee_recipient: Address::from_str(
                "0xc94f5374fce5edbc8e2a8697c15331677e6ebf0b",
            )
            .unwrap(),
            withdrawals: None,
            parent_beacon_block_root: Some(
                B256::from_str(
                    "0x2222222222222222222222222222222222222222222222222222222222222222",
                )
                .unwrap(),
            ),
        };

        // Verify that the generated payload ID matches the expected value
        assert_eq!(
            payload_id(&parent, &attributes),
            PayloadId(B64::from_str("0x0fc49cd532094cce").unwrap())
        );
    }
}
