use crate::{
    BuildArguments, BuildOutcome, MissingPayloadBehaviour, PayloadBuilder, PayloadBuilderError,
    PayloadConfig,
};

/// A stack of payload builders that allows for flexible composition of payload builders.
///
/// This structure enables the chaining of multiple `PayloadBuilder` implementations,
/// creating a hierarchical fallback system. It's designed to be nestable, allowing
/// for complex builder arrangements like `Stack<Stack<A, B>, C>`.
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

impl<L, R, Pool, Client> PayloadBuilder<Pool, Client> for PayloadBuilderStack<L, R>
where
    L: PayloadBuilder<Pool, Client>,
    R: PayloadBuilder<Pool, Client, Attributes = L::Attributes, BuiltPayload = L::BuiltPayload>,
{
    type Attributes = L::Attributes;
    type BuiltPayload = L::BuiltPayload;

    /// Attempts to build a payload using the left builder first, falling back to the right.
    fn try_build(
        &self,
        args: BuildArguments<Pool, Client, Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        let mut args = Some(args);
        match self.left.try_build(args.take().unwrap()) {
            Ok(outcome) => Ok(outcome),
            Err(_) => self.right.try_build(args.take().unwrap()),
        }
    }

    /// Handles the case where a payload is missing by delegating to the left builder first,
    /// then to the right.
    fn on_missing_payload(
        &self,
        args: BuildArguments<Pool, Client, Self::Attributes, Self::BuiltPayload>,
    ) -> MissingPayloadBehaviour<Self::BuiltPayload> {
        let mut args = Some(args);
        match self.left.on_missing_payload(args.take().unwrap()) {
            MissingPayloadBehaviour::RaceEmptyPayload => {
                self.right.on_missing_payload(args.take().unwrap())
            }
            other => other,
        }
    }

    /// Builds an empty payload using the left builder, falling back to the right.
    fn build_empty_payload(
        &self,
        client: &Client,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        let mut config = Some(config);
        self.left
            .build_empty_payload(client, config.take().unwrap())
            .or_else(|_| self.right.build_empty_payload(client, config.take().unwrap()))
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
