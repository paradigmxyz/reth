//! Actions that can be performed in tests.

use crate::testsuite::Environment;
use alloy_rpc_types_engine::{ForkchoiceUpdated, PayloadStatusEnum};
use eyre::Result;
use futures_util::future::BoxFuture;
use reth_node_api::EngineTypes;
use std::future::Future;
use tracing::debug;

pub mod fork;
pub mod produce_blocks;
pub mod reorg;

pub use fork::{CreateFork, SetForkBase, ValidateFork};
pub use produce_blocks::{
    AssertMineBlock, BroadcastLatestForkchoice, BroadcastNextNewPayload, CheckPayloadAccepted,
    GenerateNextPayload, GeneratePayloadAttributes, PickNextBlockProducer, ProduceBlocks,
    UpdateBlockInfo,
};
pub use reorg::{ReorgTarget, ReorgTo, SetReorgTarget};

/// An action that can be performed on an instance.
///
/// Actions execute operations and potentially make assertions in a single step.
/// The action name indicates what it does (e.g., `AssertMineBlock` would both
/// mine a block and assert it worked).
pub trait Action<I>: Send + 'static
where
    I: EngineTypes,
{
    /// Executes the action
    fn execute<'a>(&'a mut self, env: &'a mut Environment<I>) -> BoxFuture<'a, Result<()>>;
}

/// Simplified action container for storage in tests
#[expect(missing_debug_implementations)]
pub struct ActionBox<I>(Box<dyn Action<I>>);

impl<I> ActionBox<I>
where
    I: EngineTypes + 'static,
{
    /// Constructor for [`ActionBox`].
    pub fn new<A: Action<I>>(action: A) -> Self {
        Self(Box::new(action))
    }

    /// Executes an [`ActionBox`] with the given [`Environment`] reference.
    pub async fn execute(mut self, env: &mut Environment<I>) -> Result<()> {
        self.0.execute(env).await
    }
}

/// Implementation of `Action` for any function/closure that takes an Environment
/// reference and returns a Future resolving to Result<()>.
///
/// This allows using closures directly as actions with `.with_action(async move |env| {...})`.
impl<I, F, Fut> Action<I> for F
where
    I: EngineTypes,
    F: FnMut(&Environment<I>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<I>) -> BoxFuture<'a, Result<()>> {
        Box::pin(self(env))
    }
}

/// Run a sequence of actions in series.
#[expect(missing_debug_implementations)]
pub struct Sequence<I> {
    /// Actions to execute in sequence
    pub actions: Vec<Box<dyn Action<I>>>,
}

impl<I> Sequence<I> {
    /// Create a new sequence of actions
    pub fn new(actions: Vec<Box<dyn Action<I>>>) -> Self {
        Self { actions }
    }
}

impl<I> Action<I> for Sequence<I>
where
    I: EngineTypes + Sync + Send + 'static,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<I>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Execute each action in sequence
            for action in &mut self.actions {
                action.execute(env).await?;
            }

            Ok(())
        })
    }
}

/// Action that makes the current latest block canonical by broadcasting a forkchoice update
#[derive(Debug, Default)]
pub struct MakeCanonical {}

impl MakeCanonical {
    /// Create a new `MakeCanonical` action
    pub const fn new() -> Self {
        Self {}
    }
}

impl<Engine> Action<Engine> for MakeCanonical
where
    Engine: EngineTypes + reth_node_api::PayloadTypes,
    Engine::PayloadAttributes: From<alloy_rpc_types_engine::PayloadAttributes> + Clone,
    Engine::ExecutionPayloadEnvelopeV3:
        Into<alloy_rpc_types_engine::payload::ExecutionPayloadEnvelopeV3>,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut broadcast_action = BroadcastLatestForkchoice::default();
            broadcast_action.execute(env).await
        })
    }
}

/// Action that captures the current block and tags it with a name for later reference
#[derive(Debug)]
pub struct CaptureBlock {
    /// Tag name to associate with the current block
    pub tag: String,
}

impl CaptureBlock {
    /// Create a new `CaptureBlock` action
    pub fn new(tag: impl Into<String>) -> Self {
        Self { tag: tag.into() }
    }
}

impl<Engine> Action<Engine> for CaptureBlock
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let current_block = env
                .latest_block_info
                .as_ref()
                .ok_or_else(|| eyre::eyre!("No current block information available"))?;

            env.block_registry.insert(self.tag.clone(), current_block.hash);

            debug!(
                "Captured block {} (hash: {}) with tag '{}'",
                current_block.number, current_block.hash, self.tag
            );

            Ok(())
        })
    }
}

/// Validates a forkchoice update response and returns an error if invalid
pub fn validate_fcu_response(response: &ForkchoiceUpdated, context: &str) -> Result<()> {
    match &response.payload_status.status {
        PayloadStatusEnum::Valid => {
            debug!("{}: FCU accepted as valid", context);
            Ok(())
        }
        PayloadStatusEnum::Invalid { validation_error } => {
            Err(eyre::eyre!("{}: FCU rejected as invalid: {:?}", context, validation_error))
        }
        PayloadStatusEnum::Syncing => {
            debug!("{}: FCU accepted, node is syncing", context);
            Ok(())
        }
        PayloadStatusEnum::Accepted => {
            debug!("{}: FCU accepted for processing", context);
            Ok(())
        }
    }
}

/// Expects that the `ForkchoiceUpdated` response status is VALID.
pub fn expect_fcu_valid(response: &ForkchoiceUpdated, context: &str) -> Result<()> {
    match &response.payload_status.status {
        PayloadStatusEnum::Valid => {
            debug!("{}: FCU status is VALID as expected.", context);
            Ok(())
        }
        other_status => {
            Err(eyre::eyre!("{}: Expected FCU status VALID, but got {:?}", context, other_status))
        }
    }
}

/// Expects that the `ForkchoiceUpdated` response status is INVALID.
pub fn expect_fcu_invalid(response: &ForkchoiceUpdated, context: &str) -> Result<()> {
    match &response.payload_status.status {
        PayloadStatusEnum::Invalid { validation_error } => {
            debug!("{}: FCU status is INVALID as expected: {:?}", context, validation_error);
            Ok(())
        }
        other_status => {
            Err(eyre::eyre!("{}: Expected FCU status INVALID, but got {:?}", context, other_status))
        }
    }
}

/// Expects that the `ForkchoiceUpdated` response status is either SYNCING or ACCEPTED.
pub fn expect_fcu_syncing_or_accepted(response: &ForkchoiceUpdated, context: &str) -> Result<()> {
    match &response.payload_status.status {
        PayloadStatusEnum::Syncing => {
            debug!("{}: FCU status is SYNCING as expected (SYNCING or ACCEPTED).", context);
            Ok(())
        }
        PayloadStatusEnum::Accepted => {
            debug!("{}: FCU status is ACCEPTED as expected (SYNCING or ACCEPTED).", context);
            Ok(())
        }
        other_status => Err(eyre::eyre!(
            "{}: Expected FCU status SYNCING or ACCEPTED, but got {:?}",
            context,
            other_status
        )),
    }
}

/// Expects that the `ForkchoiceUpdated` response status is not SYNCING and not ACCEPTED.
pub fn expect_fcu_not_syncing_or_accepted(
    response: &ForkchoiceUpdated,
    context: &str,
) -> Result<()> {
    match &response.payload_status.status {
        PayloadStatusEnum::Valid => {
            debug!("{}: FCU status is VALID as expected (not SYNCING or ACCEPTED).", context);
            Ok(())
        }
        PayloadStatusEnum::Invalid { validation_error } => {
            debug!(
                "{}: FCU status is INVALID as expected (not SYNCING or ACCEPTED): {:?}",
                context, validation_error
            );
            Ok(())
        }
        syncing_or_accepted_status @ (PayloadStatusEnum::Syncing | PayloadStatusEnum::Accepted) => {
            Err(eyre::eyre!(
                "{}: Expected FCU status not SYNCING or ACCEPTED (i.e., VALID or INVALID), but got {:?}",
                context,
                syncing_or_accepted_status
            ))
        }
    }
}
