//! Mirrored version of [`ExExContext`](`crate::ExExContext`)
//! without generic abstraction over [Node](`reth_node_api::FullNodeComponents`)

use std::{fmt::Debug, sync::Arc};

use reth_chainspec::{EthChainSpec, Head};
use reth_node_api::FullNodeComponents;
use reth_node_core::node_config::NodeConfig;
use tokio::sync::mpsc;

use crate::{ExExContext, ExExEvent, ExExNotificationsStream};

// TODO(0xurb) - add `node` after abstractions
/// Captures the context that an `ExEx` has access to.
pub struct ExExContextDyn {
    /// The current head of the blockchain at launch.
    pub head: Head,
    /// The config of the node
    pub config: NodeConfig<Box<dyn EthChainSpec + 'static>>,
    /// The loaded node config
    pub reth_config: reth_config::Config,
    /// Channel used to send [`ExExEvent`]s to the rest of the node.
    ///
    /// # Important
    ///
    /// The exex should emit a `FinishedHeight` whenever a processed block is safe to prune.
    /// Additionally, the exex can pre-emptively emit a `FinishedHeight` event to specify what
    /// blocks to receive notifications for.
    pub events: mpsc::UnboundedSender<ExExEvent>,
    /// Channel to receive [`ExExNotification`](crate::ExExNotification)s.
    ///
    /// # Important
    ///
    /// Once an [`ExExNotification`](crate::ExExNotification) is sent over the channel, it is
    /// considered delivered by the node.
    pub notifications: Box<dyn ExExNotificationsStream>,
}

impl Debug for ExExContextDyn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExExContext")
            .field("head", &self.head)
            .field("config", &self.config)
            .field("reth_config", &self.reth_config)
            .field("events", &self.events)
            .field("notifications", &"...")
            .finish()
    }
}

impl<Node> From<ExExContext<Node>> for ExExContextDyn
where
    Node: FullNodeComponents,
    Node::Provider: Debug,
    Node::Executor: Debug,
{
    fn from(ctx: ExExContext<Node>) -> Self {
        // convert `NodeConfig` with generic over chainspec into `NodeConfig<Box<dyn EthChainSpec>`
        let chain: Arc<Box<dyn EthChainSpec + 'static>> =
            Arc::new(Box::new(ctx.config.chain) as Box<dyn EthChainSpec>);
        let config = NodeConfig {
            chain,
            datadir: ctx.config.datadir,
            config: ctx.config.config,
            metrics: ctx.config.metrics,
            instance: ctx.config.instance,
            network: ctx.config.network,
            rpc: ctx.config.rpc,
            txpool: ctx.config.txpool,
            builder: ctx.config.builder,
            debug: ctx.config.debug,
            db: ctx.config.db,
            dev: ctx.config.dev,
            pruning: ctx.config.pruning,
        };
        let notifications = Box::new(ctx.notifications) as Box<dyn ExExNotificationsStream>;

        Self {
            head: ctx.head,
            config,
            reth_config: ctx.reth_config,
            events: ctx.events,
            notifications,
        }
    }
}
