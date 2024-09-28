use crate::{
    BuildArguments, BuildOutcome, PayloadBuilder, PayloadBuilderError,
    PayloadConfig, PayloadBuilderAttributes
};
use futures_util::future::Either as EitherFuture;

use alloy_primitives::B256;
use reth_payload_builder::PayloadId;
use reth_payload_primitives::BuiltPayload;
use reth_primitives::{SealedBlock, U256};

use std::fmt;

/// wrapper for either error
#[derive(Debug)]
pub enum EitherError<L, R> {
    /// left error
    Left(L),
    /// right error
    Right(R),
}

impl<L, R> fmt::Display for EitherError<L, R>
where
    L: fmt::Display,
    R: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EitherError::Left(l) => write!(f, "Left error: {}", l),
            EitherError::Right(r) => write!(f, "Right error: {}", r),
        }
    }
}

impl<L, R> std::error::Error for EitherError<L, R>
where
    L: std::error::Error,
    R: std::error::Error,
{}

/// represents attributes that can be either left or right type.
#[derive(Debug, Clone)]
pub struct EitherAttributes<LAttr, RAttr>(EitherFuture<LAttr, RAttr>); 

/// implement PayloadBuilderAttributes for EitherAttributes for use in PayloadBuilderStack
impl<LAttr, RAttr> PayloadBuilderAttributes for EitherAttributes<LAttr, RAttr>
where
    LAttr: PayloadBuilderAttributes,
    RAttr: PayloadBuilderAttributes,
{
    type RpcPayloadAttributes = EitherFuture<
        LAttr::RpcPayloadAttributes,
        RAttr::RpcPayloadAttributes,
    >;
    type Error = EitherError<LAttr::Error, RAttr::Error>;

    fn try_new(
        parent: B256,
        rpc_payload_attributes: Self::RpcPayloadAttributes,
    ) -> Result<Self, Self::Error> {
        match rpc_payload_attributes {
            EitherFuture::Left(attr) => LAttr::try_new(parent, attr)
                .map(|attr| EitherAttributes(EitherFuture::Left(attr)))
                .map_err(EitherError::Left),
            EitherFuture::Right(attr) => RAttr::try_new(parent, attr)
                .map(|attr| EitherAttributes(EitherFuture::Right(attr)))
                .map_err(EitherError::Right),
        }
    }

    fn payload_id(&self) -> PayloadId {
        match &self.0 {
            EitherFuture::Left(attr) => attr.payload_id(),
            EitherFuture::Right(attr) => attr.payload_id(),
        }
    }

    fn parent(&self) -> B256 {
        match &self.0 {
            EitherFuture::Left(attr) => attr.parent(),
            EitherFuture::Right(attr) => attr.parent(),
        }
    }

    fn timestamp(&self) -> u64 {
        match &self.0 {
            EitherFuture::Left(attr) => attr.timestamp(),
            EitherFuture::Right(attr) => attr.timestamp(),
        }
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        match &self.0 {
            EitherFuture::Left(attr) => attr.parent_beacon_block_root(),
            EitherFuture::Right(attr) => attr.parent_beacon_block_root(),
        }
    }

    fn suggested_fee_recipient(&self) -> alloy_primitives::Address {
        match &self.0 {
            EitherFuture::Left(attr) => attr.suggested_fee_recipient(),
            EitherFuture::Right(attr) => attr.suggested_fee_recipient(),
        }
    }

    fn prev_randao(&self) -> B256 {
        match &self.0 {
            EitherFuture::Left(attr) => attr.prev_randao(),
            EitherFuture::Right(attr) => attr.prev_randao(),
        }
    }

    fn withdrawals(&self) -> &reth_primitives::Withdrawals {
        match &self.0 {
            EitherFuture::Left(attr) => attr.withdrawals(),
            EitherFuture::Right(attr) => attr.withdrawals(),
        }
    }
}

impl<L, R> BuiltPayload for EitherAttributes<L, R>
where
    L: BuiltPayload,
    R: BuiltPayload,
{
    fn block(&self) -> &SealedBlock {
        match &self.0 {
            EitherFuture::Left(l) => l.block(),
            EitherFuture::Right(r) => r.block(),
        }
    }

    fn fees(&self) -> U256 {
        match &self.0 {
            EitherFuture::Left(l) => l.fees(),
            EitherFuture::Right(r) => r.fees(),
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

impl<B> BuildOutcome<B> {
    fn map_payload<F, B2>(self, f: F) -> BuildOutcome<B2>
    where
        F: FnOnce(B) -> B2,
    {
        match self {
            BuildOutcome::Better { payload, cached_reads } => BuildOutcome::Better {
                payload: f(payload),
                cached_reads,
            },
            BuildOutcome::Aborted { fees, cached_reads } => BuildOutcome::Aborted { fees, cached_reads },
            BuildOutcome::Cancelled => BuildOutcome::Cancelled,
        }
    }
}

impl<L, R, Pool, Client> PayloadBuilder<Pool, Client> for PayloadBuilderStack<L, R>
where
    L: PayloadBuilder<Pool, Client>  + Unpin + 'static,
    R: PayloadBuilder<Pool, Client> + Unpin + 'static,
    Client: Clone,
    Pool: Clone,
    L::Attributes: Unpin + Clone, // Ensure Attributes can be cloned
    R::Attributes: Unpin + Clone, // Ensure Attributes can be cloned
    L::BuiltPayload: Unpin + Clone, // Ensure BuiltPayload can be cloned
    R::BuiltPayload: Unpin + Clone, // Ensure BuiltPayload can be cloned
{
    type Attributes = EitherAttributes<L::Attributes, R::Attributes>;
    type BuiltPayload = EitherAttributes<L::BuiltPayload, R::BuiltPayload>;

    /// attempts to build a payload using the left builder first, falling back to the right
    fn try_build(
        &self,
        args: BuildArguments<Pool, Client, Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        // attempt to build using the left builder
        if let EitherFuture::Left(ref left_attr) = args.config.attributes.0 {
            let left_args = BuildArguments {
                client: args.client.clone(),
                pool: args.pool.clone(),
                cached_reads: args.cached_reads.clone(),
                config: PayloadConfig {
                    parent_block: args.config.parent_block.clone(),
                    extra_data: args.config.extra_data.clone(),
                    attributes: left_attr.clone(),
                },
                cancel: args.cancel.clone(),
                best_payload: args.best_payload.clone().and_then(|p| {
                    if let EitherAttributes(EitherFuture::Left(payload)) = p {
                        Some(payload.clone())
                    } else {
                        None
                    }
                }),
            };

            // try building with the left builder
            match self.left.try_build(left_args) {
                Ok(BuildOutcome::Better { payload, cached_reads }) => {
                    return Ok(BuildOutcome::Better {
                        payload: EitherAttributes(EitherFuture::Left(payload)),
                        cached_reads,
                    });
                }
                Ok(other) => {
                    return Ok(other.map_payload(|payload| EitherAttributes(EitherFuture::Left(payload))));
                }
                Err(_) => {
                    // if left builder fails, proceed to the right builder
                }
            }
        }

        // attempt to build using the right builder
        if let EitherFuture::Right(ref right_attr) = args.config.attributes.0 {
            let right_args = BuildArguments {
                client: args.client.clone(),
                pool: args.pool.clone(),
                cached_reads: args.cached_reads.clone(),
                config: PayloadConfig {
                    parent_block: args.config.parent_block.clone(),
                    extra_data: args.config.extra_data.clone(),
                    attributes: right_attr.clone(),
                },
                cancel: args.cancel.clone(),
                best_payload: args.best_payload.and_then(|p| {
                    if let EitherAttributes(EitherFuture::Right(payload)) = p {
                        Some(payload.clone())
                    } else {
                        None
                    }
                }),
            };

            // try building with the right builder
            match self.right.try_build(right_args) {
                Ok(BuildOutcome::Better { payload, cached_reads }) => {
                    return Ok(BuildOutcome::Better {
                        payload: EitherAttributes(EitherFuture::Right(payload)),
                        cached_reads,
                    });
                }
                Ok(other) => {
                    return Ok(other.map_payload(|payload| EitherAttributes(EitherFuture::Right(payload))));
                }
                Err(err) => {
                    return Err(err);
                }
            }
        }

        // if attributes don't match Left or Right variants, return an error
        Err(PayloadBuilderError::MissingPayload)
    }

    fn build_empty_payload(
        &self,
        client: &Client,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        match config.attributes.0 {
            EitherFuture::Left(left_attr) => {
                let left_config = PayloadConfig {
                    attributes: left_attr,
                    parent_block: config.parent_block.clone(),
                    extra_data: config.extra_data.clone(),

                };
                self.left.build_empty_payload(client, left_config)
                    .map(|payload| EitherAttributes(EitherFuture::Left(payload)))
            },
            EitherFuture::Right(right_attr) => {
                let right_config = PayloadConfig {
                    attributes: right_attr,
                    parent_block: config.parent_block.clone(),
                    extra_data: config.extra_data.clone(),
                };
                self.right.build_empty_payload(client, right_config)
                    .map(|payload| EitherAttributes(EitherFuture::Right(payload)))
            },
        }
    }
}