use crate::{
    BuildArguments, BuildOutcome, HeaderForPayload, PayloadBuilder, PayloadBuilderError,
    PayloadConfig,
};

use alloy_primitives::{B256, U256};
use reth_payload_builder::PayloadId;
use reth_payload_primitives::{BuiltPayload, PayloadAttributes};
use reth_primitives_traits::{NodePrimitives, SealedBlock};

use alloy_eips::eip7685::Requests;
use std::{error::Error, fmt};

/// hand rolled Either enum to handle two builder types
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum Either<L, R> {
    /// left variant
    Left(L),
    /// right variant
    Right(R),
}

impl<L, R> fmt::Display for Either<L, R>
where
    L: fmt::Display,
    R: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Left(l) => write!(f, "Left: {l}"),
            Self::Right(r) => write!(f, "Right: {r}"),
        }
    }
}

impl<L, R> Error for Either<L, R>
where
    L: Error + 'static,
    R: Error + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Left(l) => Some(l),
            Self::Right(r) => Some(r),
        }
    }
}

impl<L, R> PayloadAttributes for Either<L, R>
where
    L: PayloadAttributes,
    R: PayloadAttributes,
{
    fn payload_id(&self, parent_hash: &B256) -> PayloadId {
        match self {
            Self::Left(l) => l.payload_id(parent_hash),
            Self::Right(r) => r.payload_id(parent_hash),
        }
    }

    fn timestamp(&self) -> u64 {
        match self {
            Self::Left(l) => l.timestamp(),
            Self::Right(r) => r.timestamp(),
        }
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        match self {
            Self::Left(l) => l.parent_beacon_block_root(),
            Self::Right(r) => r.parent_beacon_block_root(),
        }
    }

    fn withdrawals(&self) -> Option<&Vec<alloy_eips::eip4895::Withdrawal>> {
        match self {
            Self::Left(l) => l.withdrawals(),
            Self::Right(r) => r.withdrawals(),
        }
    }

    fn slot_number(&self) -> Option<u64> {
        match self {
            Self::Left(l) => l.slot_number(),
            Self::Right(r) => r.slot_number(),
        }
    }
}

/// this structure enables the chaining of multiple `PayloadBuilder` implementations,
/// creating a hierarchical fallback system. It's designed to be nestable, allowing
/// for complex builder arrangements like `Stack<Stack<A, B>, C>` with different
#[derive(Debug)]
pub struct PayloadBuilderStack<L, R> {
    left: L,
    right: R,
}

impl<L, R> PayloadBuilderStack<L, R> {
    /// Creates a new `PayloadBuilderStack` with the given left and right builders.
    pub const fn new(left: L, right: R) -> Self {
        Self { left, right }
    }
}

impl<L, R> Clone for PayloadBuilderStack<L, R>
where
    L: Clone,
    R: Clone,
{
    fn clone(&self) -> Self {
        Self::new(self.left.clone(), self.right.clone())
    }
}

impl<L, R> BuiltPayload for Either<L, R>
where
    L: BuiltPayload,
    R: BuiltPayload<Primitives = L::Primitives>,
{
    type Primitives = L::Primitives;

    fn block(&self) -> &SealedBlock<<L::Primitives as NodePrimitives>::Block> {
        match self {
            Self::Left(l) => l.block(),
            Self::Right(r) => r.block(),
        }
    }

    fn fees(&self) -> U256 {
        match self {
            Self::Left(l) => l.fees(),
            Self::Right(r) => r.fees(),
        }
    }

    fn requests(&self) -> Option<Requests> {
        match self {
            Self::Left(l) => l.requests(),
            Self::Right(r) => r.requests(),
        }
    }
}

impl<L, R> PayloadBuilder for PayloadBuilderStack<L, R>
where
    L: PayloadBuilder + Unpin + 'static,
    R: PayloadBuilder + Unpin + 'static,
    L::Attributes: Unpin + Clone,
    R::Attributes: Unpin + Clone,
    L::BuiltPayload: Unpin + Clone,
    R::BuiltPayload:
        BuiltPayload<Primitives = <L::BuiltPayload as BuiltPayload>::Primitives> + Unpin + Clone,
{
    type Attributes = Either<L::Attributes, R::Attributes>;
    type BuiltPayload = Either<L::BuiltPayload, R::BuiltPayload>;

    fn try_build(
        &self,
        args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        let BuildArguments {
            cached_reads,
            execution_cache,
            trie_handle,
            config,
            cancel,
            best_payload,
        } = args;
        let PayloadConfig { parent_header, attributes, payload_id } = config;

        match attributes {
            Either::Left(left_attr) => {
                let left_args: BuildArguments<L::Attributes, L::BuiltPayload> = BuildArguments {
                    cached_reads,
                    execution_cache,
                    trie_handle,
                    config: PayloadConfig { parent_header, attributes: left_attr, payload_id },
                    cancel,
                    best_payload: best_payload.and_then(|payload| {
                        if let Either::Left(p) = payload {
                            Some(p)
                        } else {
                            None
                        }
                    }),
                };
                self.left.try_build(left_args).map(|out| out.map_payload(Either::Left))
            }
            Either::Right(right_attr) => {
                let right_args = BuildArguments {
                    cached_reads,
                    execution_cache,
                    trie_handle,
                    config: PayloadConfig { parent_header, attributes: right_attr, payload_id },
                    cancel,
                    best_payload: best_payload.and_then(|payload| {
                        if let Either::Right(p) = payload {
                            Some(p)
                        } else {
                            None
                        }
                    }),
                };
                self.right.try_build(right_args).map(|out| out.map_payload(Either::Right))
            }
        }
    }

    fn build_empty_payload(
        &self,
        config: PayloadConfig<Self::Attributes, HeaderForPayload<Self::BuiltPayload>>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        match config {
            PayloadConfig { parent_header, attributes: Either::Left(left_attr), payload_id } => {
                let left_config =
                    PayloadConfig { parent_header, attributes: left_attr, payload_id };
                self.left.build_empty_payload(left_config).map(Either::Left)
            }
            PayloadConfig { parent_header, attributes: Either::Right(right_attr), payload_id } => {
                let right_config =
                    PayloadConfig { parent_header, attributes: right_attr, payload_id };
                self.right.build_empty_payload(right_config).map(Either::Right)
            }
        }
    }
}
