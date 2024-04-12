use futures::Future;
use reth::builder::FullNodeTypes;
use reth_exex::ExExContext;
use reth_node_ethereum::EthereumNode;
use reth_provider::CanonStateNotification;

// An ExEx can just be an async fn!
async fn exex<Node: FullNodeTypes>(mut ctx: ExExContext<Node>) -> eyre::Result<()> {
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
    }
    Ok(())
}

// The initialization logic of the ExEx is also just an async fn!
async fn exex_init<Node: FullNodeTypes>(
    ctx: ExExContext<Node>,
) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
    Ok(exex(ctx))
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("AsyncFn", exex_init)
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
