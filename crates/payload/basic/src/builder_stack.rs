use crate::{
    BuildArguments, BuildOutcome, MissingPayloadBehaviour, PayloadBuilder, PayloadBuilderError,
    PayloadConfig,
};

/// A stack of payload builders that can be used in sequence.
#[derive(Clone, Debug)]
pub struct PayloadBuilderStack<B>(Vec<B>);

impl<B> PayloadBuilderStack<B> {
    /// Create a new payload builder stack.
    pub fn new(builders: Vec<B>) -> Self {
        Self(builders)
    }

    /// Push a builder onto the stack.
    pub fn push(&mut self, builder: B) {
        self.0.push(builder);
    }
}

impl<B, Pool, Client> PayloadBuilder<Pool, Client> for PayloadBuilderStack<B>
where
    B: PayloadBuilder<Pool, Client>,
{
    type Attributes = B::Attributes;
    type BuiltPayload = B::BuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Pool, Client, Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        let mut args = Some(args);
        // Iterate through all builders in the stack
        for builder in &self.0 {
            match builder.try_build(args.take().unwrap()) {
                // If a builder produces a Better outcome, return it immediately
                Ok(outcome @ BuildOutcome::Better { .. }) => return Ok(outcome),
                // If a builder produces a Valid outcome, continue with the next builder
                Ok(_) => continue,
                // If a builder produces an Invalid outcome, continue with the next builder
                Err(_) => continue,
            }
        }
        // If no builder produced a Better outcome, return an error
        Err(PayloadBuilderError::NoBetterPayload)
    }

    fn on_missing_payload(
        &self,
        args: BuildArguments<Pool, Client, Self::Attributes, Self::BuiltPayload>,
    ) -> MissingPayloadBehaviour<Self::BuiltPayload> {
        let mut args = Some(args);
        // Find the first builder that returns a non-AwaitInProgress behaviour
        self.0
            .iter()
            .find_map(|builder| match builder.on_missing_payload(args.take().unwrap()) {
                MissingPayloadBehaviour::AwaitInProgress => None,
                behaviour => Some(behaviour),
            })
            // If all builders return AwaitInProgress, use that as the default
            .unwrap_or(MissingPayloadBehaviour::AwaitInProgress)
    }

    fn build_empty_payload(
        &self,
        client: &Client,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        let mut config = Some(config);
        // Try to build an empty payload using each builder in the stack
        self.0
            .iter()
            .find_map(|builder| builder.build_empty_payload(client, config.take().unwrap()).ok())
            // If no builder can build an empty payload, return an error
            .ok_or(PayloadBuilderError::MissingPayload)
    }
}
