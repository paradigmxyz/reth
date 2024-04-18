use futures::Future;
use reth_exex::{ExExContext, ExExEvent};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_provider::CanonStateNotification;

/// The initialization logic of the ExEx is just an async function.
///
/// During initialization you can wait for resources you need to be up for the ExEx to function,
/// like a database connection.
async fn exex_init<Node: FullNodeComponents>(
    ctx: ExExContext<Node>,
) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
    Ok(exex(ctx))
}

/// An ExEx is just a future, which means you can implement all of it in an async function!
///
/// This ExEx just prints out whenever a state transition happens, either a new chain segment being
/// added, or a chain segment being re-orged.
async fn exex<Node: FullNodeComponents>(mut ctx: ExExContext<Node>) -> eyre::Result<()> {
    while let Some(notification) = ctx.notifications.recv().await {
        match &notification {
            CanonStateNotification::Commit { new } => {
                println!("Received commit: {:?}", new.first().number..=new.tip().number);
            }
            CanonStateNotification::Reorg { old, new } => {
                println!(
                    "Received reorg: {:?} -> {:?}",
                    old.first().number..=old.tip().number,
                    new.first().number..=new.tip().number
                );
            }
        };

        ctx.events.send(ExExEvent::FinishedHeight(notification.tip().number))?;
    }
    Ok(())
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("Minimal", exex_init)
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
