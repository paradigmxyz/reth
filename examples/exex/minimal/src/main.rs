use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures::Future;
use reth::{builder::FullNodeTypes, transaction_pool::TransactionPool};
use reth_exex::{ExExContext, ExExEvent};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_provider::CanonStateNotification;

/// A minimal example of an ExEx that simply prints out commit and reorg notifications.
struct MinimalExEx<Node: FullNodeComponents> {
    ctx: ExExContext<Node>,
}

impl<Node> Future for MinimalExEx<Node>
where
    Node: FullNodeComponents,
{
    type Output = eyre::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Process all new chain state notifications until there are no more
        while let Some(notification) = ready!(this.ctx.notifications.poll_recv(cx)) {
            // Process one notification
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

            // Send a finished height event, signaling the node that we don't need any blocks below
            // this height anymore
            this.ctx.events.send(ExExEvent::FinishedHeight(notification.tip().number))?;
        }

        Poll::Pending
    }
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("Minimal", move |ctx| async { Ok(MinimalExEx { ctx }) })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
