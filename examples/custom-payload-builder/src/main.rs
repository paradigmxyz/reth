//! Example for how hook into the node via the CLI extension mechanism without registering
//! additional arguments
//!
//! Run with
//!
//! ```not_rust
//! cargo run -p custom-payload-builder -- node
//! ```
//!
//! This launch the regular reth node overriding the engine api payload builder with our custom.
use clap::Parser;
use generator::EmptyBlockPayloadJobGenerator;
use reth::{
    cli::{
        components::RethNodeComponents,
        config::PayloadBuilderConfig,
        ext::{NoArgsCliExt, RethNodeCommandConfig},
        Cli,
    },
    payload::PayloadBuilderHandle,
    providers::CanonStateSubscriptions,
    tasks::TaskSpawner,
};
use reth_basic_payload_builder::{BasicPayloadJobGeneratorConfig, PayloadBuilder};
use reth_node_api::EngineTypes;
use reth_payload_builder::PayloadBuilderService;

pub mod generator;
pub mod job;

fn main() {
    Cli::<NoArgsCliExt<MyCustomBuilder>>::parse()
        .with_node_extension(MyCustomBuilder::default())
        .run()
        .unwrap();
}

/// Our custom cli args extension that adds one flag to reth default CLI.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
struct MyCustomBuilder;

impl RethNodeCommandConfig for MyCustomBuilder {
    fn spawn_payload_builder_service<Conf, Reth, Builder, Engine>(
        &mut self,
        conf: &Conf,
        components: &Reth,
        payload_builder: Builder,
    ) -> eyre::Result<PayloadBuilderHandle<Engine>>
    where
        Conf: PayloadBuilderConfig,
        Reth: RethNodeComponents,
        Engine: EngineTypes + 'static,
        Builder: PayloadBuilder<
                Reth::Pool,
                Reth::Provider,
                Attributes = Engine::PayloadBuilderAttributes,
                BuiltPayload = Engine::BuiltPayload,
            > + Unpin
            + 'static,
    {
        tracing::info!("Spawning a custom payload builder");

        let payload_job_config = BasicPayloadJobGeneratorConfig::default()
            .interval(conf.interval())
            .deadline(conf.deadline())
            .max_payload_tasks(conf.max_payload_tasks())
            .extradata(conf.extradata_rlp_bytes())
            .max_gas_limit(conf.max_gas_limit());

        let payload_generator = EmptyBlockPayloadJobGenerator::with_builder(
            components.provider(),
            components.pool(),
            components.task_executor(),
            payload_job_config,
            components.chain_spec().clone(),
            payload_builder,
        );

        let (payload_service, payload_builder) = PayloadBuilderService::new(
            payload_generator,
            components.events().canonical_state_stream(),
        );

        components
            .task_executor()
            .spawn_critical("custom payload builder service", Box::pin(payload_service));

        Ok(payload_builder)
    }
}
