use eyre::OptionExt;
use reth::{
    providers::{DatabaseProviderFactory, HistoricalStateProviderRef},
    revm::database::StateProviderDatabase,
};
use reth_evm::execute::{BatchExecutor, BlockExecutorProvider};
use reth_evm_ethereum::EthEvmConfig;
use reth_exex::{ExExContext, ExExEvent};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::{EthExecutorProvider, EthereumNode};
use reth_tracing::tracing::info;

async fn exex<Node: FullNodeComponents>(mut ctx: ExExContext<Node>) -> eyre::Result<()> {
    while let Some(notification) = ctx.notifications.recv().await {
        if let Some(chain) = notification.committed_chain() {
            // TODO(alexey): use custom EVM config with tracer
            let evm_config = EthEvmConfig::default();
            let executor_provider = EthExecutorProvider::new(ctx.config.chain.clone(), evm_config);

            let database_provider = ctx.provider().database_provider_ro()?;
            let db = StateProviderDatabase::new(HistoricalStateProviderRef::new(
                database_provider.tx_ref(),
                chain.first().number.checked_sub(1).ok_or_eyre("block number underflow")?,
                database_provider.static_file_provider().clone(),
            ));

            let mut executor = executor_provider.batch_executor(
                db,
                ctx.config.prune_config().map(|config| config.segments).unwrap_or_default(),
            );

            for block in chain.blocks_iter() {
                let td = block.header().difficulty;
                executor.execute_one((&block.clone().unseal(), td).into())?;
            }

            let output = executor.finalize();

            let same_state = chain.state() == &output.into();
            info!(
                chain = ?chain.range(),
                %same_state,
                "Executed chain"
            );

            ctx.events.send(ExExEvent::FinishedHeight(chain.tip().number))?;
        }
    }
    Ok(())
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("EVM", |ctx| async move { Ok(exex(ctx)) })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
