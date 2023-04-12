//! Contains types required for building a payload.

use reth_primitives::{Address, SealedBlock, Withdrawal, H256, U256};
use reth_rlp::Encodable;
use reth_rpc_types::engine::{PayloadAttributes, PayloadId};

/// Contains the built payload.
///
/// According to the [engine API specification](https://github.com/ethereum/execution-apis/blob/main/src/engine/README.md) the execution layer should build the initial version of the payload with an empty transaction set and then keep update it in order to maximize the revenue.
/// Therefore, the empty-block here is always available and full-block will be set/updated
/// afterwards.
#[derive(Debug)]
pub struct BuiltPayload {
    /// Identifier of the payload
    pub(crate) id: PayloadId,
    /// The built block
    pub(crate) block: SealedBlock,
    /// The fees of the block
    pub(crate) fees: U256,
}

// === impl BuiltPayload ===

impl BuiltPayload {
    /// Initializes the payload with the given initial block.
    pub fn new(id: PayloadId, block: SealedBlock, fees: U256) -> Self {
        Self { id, block, fees }
    }

    /// Returns the identifier of the payload.
    pub fn id(&self) -> PayloadId {
        self.id
    }

    /// Returns the identifier of the payload.
    pub fn block(&self) -> &SealedBlock {
        &self.block
    }

    /// Fees of the block
    pub fn fees(&self) -> U256 {
        self.fees
    }
}

/// Container type for all components required to build a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadBuilderAttributes {
    /// Id of the payload
    pub id: PayloadId,
    /// Parent block to build the payload on top
    pub parent: H256,
    /// Timestamp for the generated payload
    pub timestamp: u64,
    /// Address of the recipient for collecting transaction fee
    pub suggested_fee_recipient: Address,
    /// Randomness value for the generated payload
    pub prev_randao: H256,
    /// Withdrawals for the generated payload
    pub withdrawals: Vec<Withdrawal>,
}

// === impl PayloadBuilderAttributes ===

impl PayloadBuilderAttributes {
    /// Creates a new payload builder for the given parent block and the attributes.
    ///
    /// Derives the unique [PayloadId] for the given parent and attributes
    pub fn new(parent: H256, attributes: PayloadAttributes) -> Self {
        let id = payload_id(&parent, &attributes);
        Self {
            id,
            parent,
            timestamp: attributes.timestamp.as_u64(),
            suggested_fee_recipient: attributes.suggested_fee_recipient,
            prev_randao: attributes.prev_randao,
            withdrawals: attributes.withdrawals.unwrap_or_default(),
        }
    }

    /// Returns the identifier of the payload.
    pub fn payload_id(&self) -> PayloadId {
        self.id
    }
}

/// Generates the payload id for the configured payload
///
/// Returns an 8-byte identifier by hashing the payload components.
pub(crate) fn payload_id(parent: &H256, attributes: &PayloadAttributes) -> PayloadId {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(parent.as_bytes());
    hasher.update(&attributes.timestamp.as_u64().to_be_bytes()[..]);
    hasher.update(attributes.prev_randao.as_bytes());
    hasher.update(attributes.suggested_fee_recipient.as_bytes());
    if let Some(withdrawals) = &attributes.withdrawals {
        let mut buf = Vec::new();
        withdrawals.encode(&mut buf);
        hasher.update(buf);
    }
    let out = hasher.finalize();
    PayloadId::new(out.as_slice()[..8].try_into().expect("sufficient length"))
}
