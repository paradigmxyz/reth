//! Example for how hook into the node via the CLI extension mechanism without registering
//! additional arguments
//!
//! Run with
//!
//! ```not_rust
//! cargo run -p cli-extension-event-hooks -- node
//! ```
//!
//! This launch the regular reth node and also print:
//!
//! > "All components initialized"
//! once all components have been initialized and
//!
//! > "Node started"
//! once the node has been started.
use clap::Parser;
use reth::cli::{
    components::RethNodeComponents,
    ext::{NoArgsCliExt, RethNodeCommandConfig},
    Cli,
};

fn main() {
    Cli::<NoArgsCliExt<MyRethConfig>>::parse()
        .with_node_extension(MyRethConfig::default())
        .run()
        .unwrap();
}

/// Our custom cli args extension that adds one flag to reth default CLI.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
struct MyRethConfig;

impl RethNodeCommandConfig for MyRethConfig {
    fn on_components_initialized<Reth: RethNodeComponents>(
        &mut self,
        _components: &Reth,
    ) -> eyre::Result<()> {
        println!("All components initialized");
        Ok(())
    }
    fn on_node_started<Reth: RethNodeComponents>(
        &mut self,
        _components: &Reth,
    ) -> eyre::Result<()> {
        println!("Node started");
        Ok(())
    }
}
