use std::fmt;

use reth_node_api::{FullNodeComponents, FullNodeComponentsExt};

use crate::{
    common::{ExtComponentBuilder, InitializedComponents, LaunchContextExt},
    node::FullNode,
    NodeAddOnsExt,
};

/// Container for all the configurable hook functions.
pub(crate) struct NodeHooks<Node> {
    pub(crate) on_component_initialized: Box<dyn OnComponentsInitializedHook<Node>>,
    pub(crate) on_node_started: Box<dyn OnNodeStartedHook<Node>>,
    pub(crate) _marker: std::marker::PhantomData<Node>,
}

impl<Node: FullNodeComponents> NodeHooks<Node> {
    /// Sets the hook that is run once the node's components are initialized.
    #[allow(unused)]
    pub(crate) fn on_components_initialized<F>(mut self, hook: F) -> NodeHooks<Node>
    where
        F: OnComponentsInitializedHook<Node> + 'static,
    {
        let Self { on_node_started, _marker, .. } = self;
        NodeHooks { on_component_initialized: Box::new(hook), on_node_started, _marker }
    }

    /// Sets the hook that is run once the node has started.
    pub(crate) fn set_on_node_started<F>(&mut self, hook: F) -> &mut Self
    where
        F: OnNodeStartedHook<Node> + 'static,
    {
        self.on_node_started = Box::new(hook);
        self
    }

    /// Sets the hook that is run once the node has started.
    #[allow(unused)]
    pub(crate) fn on_node_started<F>(mut self, hook: F) -> Self
    where
        F: OnNodeStartedHook<Node> + 'static,
    {
        self.set_on_node_started(hook);
        self
    }
}

impl<Node: FullNodeComponents> Default for NodeHooks<Node> {
    fn default() -> Self {
        Self {
            on_component_initialized: Box::<()>::default(),
            on_node_started: Box::<()>::default(),
            _marker: Default::default(),
        }
    }
}
impl<Node: FullNodeComponents> fmt::Debug for NodeHooks<Node> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeHooks")
            .field("on_component_initialized", &"...")
            .field("on_node_started", &"...")
            .finish()
    }
}

/// A helper trait for the event hook that is run once the node core components are initialized.
pub trait OnComponentsInitializedHook<Node: FullNodeComponentsExt>: Send {
    /// Consumes the event hook and runs it.
    ///
    /// If this returns an error, the node launch will be aborted.
    fn on_event(
        self: Box<Self>,
        ctx: &mut LaunchContextExt<Node>,
        hooks: NodeAddOnsExt<Node>,
    ) -> eyre::Result<()>;
}

impl<Node, F> OnComponentsInitializedHook<Node> for F
where
    Node: FullNodeComponentsExt,
    F: FnOnce(&mut LaunchContextExt<Node>, NodeAddOnsExt<Node>) -> eyre::Result<()> + Send,
{
    fn on_event(
        self: Box<Self>,
        ctx: &mut LaunchContextExt<Node>,
        hooks: NodeAddOnsExt<Node>,
    ) -> eyre::Result<()> {
        (*self)(ctx, node, hooks)
    }
}

impl<Node, F> OnComponentsInitializedHook<Node> for F
where
    Node: FullNodeComponentsExt,
    F: FnOnce(
        &mut LaunchContextExt<Node>,
    ) -> impl FnOnce(&mut LaunchContextExt<Node>, NodeAddOnsExt<Node>),
{
    fn on_event(
        self: Box<Self>,
        ctx: &mut LaunchContextExt<Node>,
        hooks: NodeAddOnsExt<Node>,
    ) -> eyre::Result<()> {
        (*self)(ctx)(ctx, hooks)
    }
}

/// A helper trait that is run once the node is started.
pub trait OnNodeStartedHook<Node: FullNodeComponentsExt>: Send {
    /// Consumes the event hook and runs it.
    ///
    /// If this returns an error, the node launch will be aborted.
    fn on_event(self: Box<Self>, node: FullNode<Node>) -> eyre::Result<()>;
}

impl<Node, F> OnNodeStartedHook<Node> for F
where
    Node: FullNodeComponentsExt,
    F: FnOnce(FullNode<Node>) -> eyre::Result<()> + Send,
{
    fn on_event(self: Box<Self>, node: FullNode<Node>) -> eyre::Result<()> {
        (*self)(node)
    }
}

impl<Node: FullNodeComponentsExt> OnComponentsInitializedHook<Node> for () {
    fn on_event(
        self: Box<Self>,
        _ctx: LaunchContextExt<Node>,
        _hooks: NodeAddOnsExt<Node>,
    ) -> eyre::Result<()> {
        Ok(())
    }
}

impl<Node: FullNodeComponentsExt> OnNodeStartedHook<Node> for () {
    fn on_event(self: Box<Self>, _node: FullNode<Node>) -> eyre::Result<()> {
        Ok(())
    }
}
