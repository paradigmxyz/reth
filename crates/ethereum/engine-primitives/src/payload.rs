//! Contains types required for building a payload.

use alloc::{sync::Arc, vec::Vec};
use alloy_eips::{
    eip4844::BlobTransactionSidecar,
    eip7594::{BlobTransactionSidecarEip7594, BlobTransactionSidecarVariant},
    eip7685::Requests,
};
use alloy_primitives::{Bytes, U256};
use alloy_rpc_types_engine::{
    BlobsBundleV1, BlobsBundleV2, ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3,
    ExecutionPayloadEnvelopeV4, ExecutionPayloadEnvelopeV5, ExecutionPayloadEnvelopeV6,
    ExecutionPayloadFieldV2, ExecutionPayloadV1, ExecutionPayloadV3, ExecutionPayloadV4,
};
use reth_ethereum_primitives::EthPrimitives;
use reth_payload_primitives::BuiltPayload;
use reth_primitives_traits::{NodePrimitives, SealedBlock};

use crate::BuiltPayloadConversionError;

/// Contains the built payload.
///
/// According to the [engine API specification](https://github.com/ethereum/execution-apis/blob/main/src/engine/README.md) the execution layer should build the initial version of the payload with an empty transaction set and then keep update it in order to maximize the revenue.
///
/// This struct represents a single built block at a point in time. The payload building process
/// creates a sequence of these payloads, starting with an empty block and progressively including
/// more transactions.
#[derive(Debug, Clone)]
pub struct EthBuiltPayload<N: NodePrimitives = EthPrimitives> {
    /// The built block
    pub(crate) block: Arc<SealedBlock<N::Block>>,
    /// The fees of the block
    pub(crate) fees: U256,
    /// The blobs, proofs, and commitments in the block. If the block is pre-cancun, this will be
    /// empty.
    pub(crate) sidecars: BlobSidecars,
    /// The requests of the payload
    pub(crate) requests: Option<Requests>,
    /// The block access list of the payload
    pub(crate) block_access_list: Option<Bytes>,
}

// === impl BuiltPayload ===

impl<N: NodePrimitives> EthBuiltPayload<N> {
    /// Initializes the payload with the given initial block
    ///
    /// Caution: This does not set any [`BlobSidecars`].
    pub const fn new(
        block: Arc<SealedBlock<N::Block>>,
        fees: U256,
        requests: Option<Requests>,
        block_access_list: Option<Bytes>,
    ) -> Self {
        Self { block, fees, requests, sidecars: BlobSidecars::Empty, block_access_list }
    }

    /// Returns the built block(sealed)
    pub fn block(&self) -> &SealedBlock<N::Block> {
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
}

impl EthBuiltPayload {
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
    pub fn try_into_v4(
        mut self,
    ) -> Result<ExecutionPayloadEnvelopeV4, BuiltPayloadConversionError> {
        let execution_requests = self.requests.take().unwrap_or_default();
        Ok(ExecutionPayloadEnvelopeV4 { execution_requests, envelope_inner: self.try_into()? })
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

    /// Try converting built payload into [`ExecutionPayloadEnvelopeV6`].
    ///
    /// Returns an error if the block access list is missing, as it's required for V6 envelopes.
    pub fn try_into_v6(self) -> Result<ExecutionPayloadEnvelopeV6, BuiltPayloadConversionError> {
        let Self { block, fees, sidecars, requests, block_access_list, .. } = self;

        let block_access_list =
            block_access_list.ok_or(BuiltPayloadConversionError::MissingBlockAccessList)?;

        let blobs_bundle = match sidecars {
            BlobSidecars::Empty => BlobsBundleV2::empty(),
            BlobSidecars::Eip7594(sidecars) => BlobsBundleV2::from(sidecars),
            BlobSidecars::Eip4844(_) => {
                return Err(BuiltPayloadConversionError::UnexpectedEip4844Sidecars)
            }
        };
        Ok(ExecutionPayloadEnvelopeV6 {
            execution_payload: ExecutionPayloadV4::from_block_unchecked_with_bal(
                block.hash(),
                &Arc::unwrap_or_clone(block).into_block(),
                block_access_list,
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

impl<N: NodePrimitives> BuiltPayload for EthBuiltPayload<N> {
    type Primitives = N;

    fn block(&self) -> &SealedBlock<N::Block> {
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

impl TryFrom<EthBuiltPayload> for ExecutionPayloadEnvelopeV6 {
    type Error = BuiltPayloadConversionError;

    fn try_from(value: EthBuiltPayload) -> Result<Self, Self::Error> {
        value.try_into_v6()
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
