use crate::{ExExContextDyn, ExExEvent, ExExNotifications, ExExNotificationsStream};
use reth_exex_types::ExExHead;
use reth_node_api::{FullNodeComponents, NodeTypes};
use reth_node_core::node_config::NodeConfig;
use reth_primitives::Head;
use reth_provider::BlockReader;
use reth_tasks::TaskExecutor;
use std::fmt::Debug;
use tokio::sync::mpsc::UnboundedSender;

/// Captures the context that an `ExEx` has access to.
pub struct ExExContext<Node: FullNodeComponents> {
    /// The current head of the blockchain at launch.
    pub head: Head,
    /// The config of the node
    pub config: NodeConfig<<Node::Types as NodeTypes>::ChainSpec>,
    /// The loaded node config
    pub reth_config: reth_config::Config,
    /// Channel used to send [`ExExEvent`]s to the rest of the node.
    ///
    /// # Important
    ///
    /// The exex should emit a `FinishedHeight` whenever a processed block is safe to prune.
    /// Additionally, the exex can pre-emptively emit a `FinishedHeight` event to specify what
    /// blocks to receive notifications for.
    pub events: UnboundedSender<ExExEvent>,
    /// Channel to receive [`ExExNotification`](crate::ExExNotification)s.
    ///
    /// # Important
    ///
    /// Once an [`ExExNotification`](crate::ExExNotification) is sent over the channel, it is
    /// considered delivered by the node.
    pub notifications: ExExNotifications<Node::Provider, Node::Executor>,

    /// Node components
    pub components: Node,
}

impl<Node> Debug for ExExContext<Node>
where
    Node: FullNodeComponents,
    Node::Provider: Debug,
    Node::Executor: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExExContext")
            .field("head", &self.head)
            .field("config", &self.config)
            .field("reth_config", &self.reth_config)
            .field("events", &self.events)
            .field("notifications", &self.notifications)
            .field("components", &"...")
            .finish()
    }
}

impl<Node> ExExContext<Node>
where
    Node: FullNodeComponents,
    Node::Provider: Debug + BlockReader<Block = reth_primitives::Block>,
    Node::Executor: Debug,
{
    /// Returns dynamic version of the context
    pub fn into_dyn(self) -> ExExContextDyn {
        ExExContextDyn::from(self)
    }
}

impl<Node> ExExContext<Node>
where
    Node: FullNodeComponents,
{
    /// Returns the transaction pool of the node.
    pub fn pool(&self) -> &Node::Pool {
        self.components.pool()
    }

    /// Returns the node's evm config.
    pub fn evm_config(&self) -> &Node::Evm {
        self.components.evm_config()
    }

    /// Returns the node's executor type.
    pub fn block_executor(&self) -> &Node::Executor {
        self.components.block_executor()
    }

    /// Returns the provider of the node.
    pub fn provider(&self) -> &Node::Provider {
        self.components.provider()
    }

    /// Returns the handle to the network
    pub fn network(&self) -> &Node::Network {
        self.components.network()
    }

    /// Returns the handle to the payload builder service.
    pub fn payload_builder(&self) -> &Node::PayloadBuilder {
        self.components.payload_builder()
    }

    /// Returns the task executor.
    pub fn task_executor(&self) -> &TaskExecutor {
        self.components.task_executor()
    }

    /// Sets notifications stream to [`crate::ExExNotificationsWithoutHead`], a stream of
    /// notifications without a head.
    pub fn set_notifications_without_head(&mut self)
    where
        Node::Provider: BlockReader<Block = reth_primitives::Block>,
    {
        self.notifications.set_without_head();
    }

    /// Sets notifications stream to [`crate::ExExNotificationsWithHead`], a stream of notifications
    /// with the provided head.
    pub fn set_notifications_with_head(&mut self, head: ExExHead)
    where
        Node::Provider: BlockReader<Block = reth_primitives::Block>,
    {
        self.notifications.set_with_head(head);
    }
}

#[cfg(test)]
mod tests {
    use reth_exex_types::ExExHead;
    use reth_node_api::FullNodeComponents;
    use reth_provider::BlockReader;

    use crate::ExExContext;

    /// <https://github.com/paradigmxyz/reth/issues/12054>
    #[test]
    const fn issue_12054() {
        #[allow(dead_code)]
        struct ExEx<Node: FullNodeComponents> {
            ctx: ExExContext<Node>,
        }

        impl<Node: FullNodeComponents> ExEx<Node>
        where
            Node::Provider: BlockReader<Block = reth_primitives::Block>,
        {
            async fn _test_bounds(mut self) -> eyre::Result<()> {
                self.ctx.pool();
                self.ctx.block_executor();
                self.ctx.provider();
                self.ctx.network();
                self.ctx.payload_builder();
                self.ctx.task_executor();
                self.ctx.set_notifications_without_head();
                self.ctx.set_notifications_with_head(ExExHead { block: Default::default() });
                Ok(())
            }
        }
    }
}
