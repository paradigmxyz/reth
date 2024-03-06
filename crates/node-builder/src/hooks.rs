use crate::{components::FullNodeComponents, node::FullNode};
use std::fmt;

/// Container for all the configurable hook functions.
pub(crate) struct NodeHooks<Node: FullNodeComponents> {
    pub(crate) on_component_initialized: Box<dyn OnComponentInitializedHook<Node>>,
    pub(crate) on_node_started: Box<dyn OnNodeStartedHook<Node>>,
    pub(crate) _marker: std::marker::PhantomData<Node>,
}

impl<Node: FullNodeComponents> NodeHooks<Node> {
    /// Creates a new, empty [NodeHooks] instance for the given node type.
    pub(crate) fn new() -> Self {
        Self {
            on_component_initialized: Box::<()>::default(),
            on_node_started: Box::<()>::default(),
            _marker: Default::default(),
        }
    }

    /// Sets the hook that is run once the node's components are initialized.
    pub(crate) fn set_on_component_initialized<F>(&mut self, hook: F) -> &mut Self
    where
        F: OnComponentInitializedHook<Node> + 'static,
    {
        self.on_component_initialized = Box::new(hook);
        self
    }

    /// Sets the hook that is run once the node's components are initialized.
    #[allow(unused)]
    pub(crate) fn on_component_initialized<F>(mut self, hook: F) -> Self
    where
        F: OnComponentInitializedHook<Node> + 'static,
    {
        self.set_on_component_initialized(hook);
        self
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
        Self::new()
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

/// A helper trait for the event hook that is run once the node is initialized.
pub trait OnComponentInitializedHook<Node>: Send {
    /// Consumes the event hook and runs it.
    ///
    /// If this returns an error, the node launch will be aborted.
    fn on_event(&self, node: Node) -> eyre::Result<()>;
}

impl<Node, F> OnComponentInitializedHook<Node> for F
where
    F: Fn(Node) -> eyre::Result<()> + Send,
{
    fn on_event(&self, node: Node) -> eyre::Result<()> {
        self(node)
    }
}

/// A helper trait that is run once the node is started.
pub trait OnNodeStartedHook<Node: FullNodeComponents>: Send {
    /// Consumes the event hook and runs it.
    ///
    /// If this returns an error, the node launch will be aborted.
    fn on_event(&self, node: FullNode<Node>) -> eyre::Result<()>;
}

impl<Node, F> OnNodeStartedHook<Node> for F
where
    Node: FullNodeComponents,
    F: Fn(FullNode<Node>) -> eyre::Result<()> + Send,
{
    fn on_event(&self, node: FullNode<Node>) -> eyre::Result<()> {
        self(node)
    }
}

impl<Node> OnComponentInitializedHook<Node> for () {
    fn on_event(&self, _node: Node) -> eyre::Result<()> {
        Ok(())
    }
}

impl<Node: FullNodeComponents> OnNodeStartedHook<Node> for () {
    fn on_event(&self, _node: FullNode<Node>) -> eyre::Result<()> {
        Ok(())
    }
}
