//! Contains types required for building a payload.

use reth_primitives::{Address, Block, SealedBlock, Withdrawal, H256};
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
    /// The initially empty block.
    _initial_empty_block: SealedBlock,
}

// === impl BuiltPayload ===

impl BuiltPayload {
    /// Initializes the payload with the given initial block.
    pub(crate) fn new(id: PayloadId, initial: Block) -> Self {
        Self { id, _initial_empty_block: initial.seal_slow() }
    }

    /// Returns the identifier of the payload.
    pub fn id(&self) -> PayloadId {
        self.id
    }
}

/// Container type for all components required to build a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadBuilderAttributes {
    /// Parent block to build the payload on top
    pub(crate) parent: H256,
    /// Timestamp for the generated payload
    pub(crate) timestamp: u64,
    /// Address of the recipient for collecting transaction fee
    pub(crate) suggested_fee_recipient: Address,
    /// Randomness value for the generated payload
    pub(crate) prev_randao: H256,
    /// Withdrawals for the generated payload
    pub(crate) withdrawals: Vec<Withdrawal>,
}

// === impl PayloadBuilderAttributes ===

impl PayloadBuilderAttributes {
    /// Creates a new payload builder for the given parent block and the attributes
    pub fn new(parent: H256, attributes: PayloadAttributes) -> Self {
        Self {
            parent,
            timestamp: attributes.timestamp.as_u64(),
            suggested_fee_recipient: attributes.suggested_fee_recipient,
            prev_randao: attributes.prev_randao,
            withdrawals: attributes.withdrawals.unwrap_or_default(),
        }
    }

    /// Generates the payload id for the configured payload
    ///
    /// Returns an 8-byte identifier by hashing the payload components.
    pub fn payload_id(&self) -> PayloadId {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(self.parent.as_bytes());
        hasher.update(&self.timestamp.to_be_bytes()[..]);
        hasher.update(self.prev_randao.as_bytes());
        hasher.update(self.suggested_fee_recipient.as_bytes());
        let mut buf = Vec::new();
        self.withdrawals.encode(&mut buf);
        hasher.update(buf);
        let out = hasher.finalize();
        PayloadId::new(out.as_slice()[..8].try_into().expect("sufficient length"))
    }
}
