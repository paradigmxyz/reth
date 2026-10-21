#![allow(missing_docs)]

use clap::Parser;
use reth_chainspec::ChainSpec;
use reth_cli::chainspec::{parse_genesis, ChainSpecParser};
use reth_cli_commands::{launcher::FnLauncher, node::NodeCommand};
use reth_cli_runner::CliRunner;
use reth_pureth_block::ExecutionHeader;
use reth_pureth_node::ExecutionNode;
use reth_tracing::{RethTracer, Tracer};
use std::{num::NonZeroU32, sync::Arc};

#[derive(Clone)]
struct ExecutionChainParser;

impl ChainSpecParser for ExecutionChainParser {
    type ChainSpec = ChainSpec<ExecutionHeader>;
    const SUPPORTED_CHAINS: &'static [&'static str] = &[];

    fn parse(input: &str) -> eyre::Result<Arc<Self::ChainSpec>> {
        let native = Arc::new(ChainSpec::from(parse_genesis(input)?));
        ExecutionNode::new(native.clone(), NonZeroU32::MIN)?;
        Ok(Arc::new(native.as_ref().clone().map_header(ExecutionHeader::legacy)))
    }
}

#[derive(Debug, clap::Args)]
struct RuntimeArgs {
    #[arg(long = "pureth.source-cache-entries")]
    source_cache_entries: NonZeroU32,
}

fn main() -> eyre::Result<()> {
    reth_primitives_traits::ensure_no_account_extensions()?;
    let command = NodeCommand::<ExecutionChainParser, RuntimeArgs>::parse();
    eyre::ensure!(
        !command.dev.dev,
        "use a scripted Engine driver; automatic dev mining is unsupported"
    );
    eyre::ensure!(!command.jit.enabled, "this execution configuration uses the interpreter");
    let native =
        Arc::new(command.chain.as_ref().clone().map_header(|header| header.as_native().clone()));
    let node = ExecutionNode::new(native, command.ext.source_cache_entries)?;
    let _guard = RethTracer::new().init()?;
    CliRunner::try_default_runtime()?.run_command_until_exit(|ctx| {
        command.execute(
            ctx,
            FnLauncher::new::<ExecutionChainParser, RuntimeArgs>(async move |builder, _args| {
                let handle = builder.node(node).launch().await?;
                handle.wait_for_node_exit().await
            }),
        )
    })
}
