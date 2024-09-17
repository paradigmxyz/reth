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
        for builder in &self.0 {
            match builder.try_build(args.take().unwrap()) {
                Ok(outcome @ BuildOutcome::Better { .. }) => return Ok(outcome),
                Ok(_) => continue,
                Err(_) => continue,
            }
        }
        Err(PayloadBuilderError::NoBetterPayload)
    }

    fn on_missing_payload(
        &self,
        args: BuildArguments<Pool, Client, Self::Attributes, Self::BuiltPayload>,
    ) -> MissingPayloadBehaviour<Self::BuiltPayload> {
        let mut args = Some(args);
        self.0
            .iter()
            .find_map(|builder| match builder.on_missing_payload(args.take().unwrap()) {
                MissingPayloadBehaviour::AwaitInProgress => None,
                behaviour => Some(behaviour),
            })
            .unwrap_or(MissingPayloadBehaviour::AwaitInProgress)
    }

    fn build_empty_payload(
        &self,
        client: &Client,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        let mut config = Some(config);
        self.0
            .iter()
            .find_map(|builder| builder.build_empty_payload(client, config.take().unwrap()).ok())
            .ok_or(PayloadBuilderError::MissingPayload)
    }
}
