use std::{fmt, pin::Pin};

use futures::Future;
use reth_node_api::{FullNodeComponents, FullNodeComponentsExt};

use crate::{common::ExtBuilderContext, node::FullNode};

/// Container for all the configurable hook functions.
pub(crate) struct NodeHooks<Node> {
    pub(crate) on_components_initialized: Vec<Box<dyn OnComponentsInitializedHook<Node>>>,
    pub(crate) on_node_started: Box<dyn OnNodeStartedHook<Node>>,
    pub(crate) _marker: std::marker::PhantomData<Node>,
}

impl<Node: FullNodeComponentsExt> NodeHooks<Node> {
    /// Sets a hook that is run once the node's components are initialized.
    pub(crate) fn on_components_initialized<F>(self, hook: F) -> NodeHooks<Node>
    where
        F: OnComponentsInitializedHook<Node> + 'static,
    {
        let Self { on_node_started, _marker, mut on_components_initialized } = self;
        on_components_initialized.push(Box::new(hook));
        NodeHooks { on_components_initialized, on_node_started, _marker }
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

impl<Node: FullNodeComponentsExt> Default for NodeHooks<Node> {
    fn default() -> Self {
        Self {
            on_components_initialized: vec![Box::<()>::default()],
            on_node_started: Box::<()>::default(),
            _marker: Default::default(),
        }
    }
}
impl<Node: FullNodeComponents> fmt::Debug for NodeHooks<Node> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeHooks")
            .field("on_components_initialized", &"...")
            .field("on_node_started", &"...")
            .finish()
    }
}

/// A helper trait for the event hook that is run once the node core components are initialized.
pub trait OnComponentsInitializedHook<Node: FullNodeComponentsExt>: Send {
    /// Consumes the event hook and runs it.
    ///
    /// If this returns an error, the node launch will be aborted.
    fn on_event<'a>(
        self: Box<Self>,
        ctx: ExtBuilderContext<'a, Node>,
    ) -> Pin<Box<dyn Future<Output = eyre::Result<()>> + Send>>;
}

impl<Node, F> OnComponentsInitializedHook<Node> for F
where
    Node: FullNodeComponentsExt,
    for<'a> F: FnOnce(
            ExtBuilderContext<'a, Node>,
        ) -> Pin<Box<dyn Future<Output = eyre::Result<()>> + Send>>
        + Send,
{
    fn on_event<'a>(
        self: Box<Self>,
        ctx: ExtBuilderContext<'a, Node>,
    ) -> Pin<Box<dyn Future<Output = eyre::Result<()>> + Send>> {
        (*self)(ctx)
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

impl<Node: FullNodeComponentsExt> OnNodeStartedHook<Node> for () {
    fn on_event(self: Box<Self>, _node: FullNode<Node>) -> eyre::Result<()> {
        Ok(())
    }
}
