use std::fmt;

use reth_node_api::{FullNodeComponents, NodeAddOns};

use crate::node::FullNode;

/// Container for all the configurable hook functions.
pub struct NodeHooks<Node: FullNodeComponents, AddOns: NodeAddOns<Node>> {
    /// Hook to run once core components are initialized.
    pub on_component_initialized: Box<dyn OnComponentInitializedHook<Node>>,
    /// Hook to run once the node is started.
    pub on_node_started: Box<dyn OnNodeStartedHook<Node, AddOns>>,
    _marker: std::marker::PhantomData<Node>,
}

impl<Node, AddOns> NodeHooks<Node, AddOns>
where
    Node: FullNodeComponents,
    AddOns: NodeAddOns<Node>,
{
    /// Creates a new, empty [`NodeHooks`] instance for the given node type.
    pub fn new() -> Self {
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
        F: OnNodeStartedHook<Node, AddOns> + 'static,
    {
        self.on_node_started = Box::new(hook);
        self
    }

    /// Sets the hook that is run once the node has started.
    #[allow(unused)]
    pub(crate) fn on_node_started<F>(mut self, hook: F) -> Self
    where
        F: OnNodeStartedHook<Node, AddOns> + 'static,
    {
        self.set_on_node_started(hook);
        self
    }
}

impl<Node, AddOns> Default for NodeHooks<Node, AddOns>
where
    Node: FullNodeComponents,
    AddOns: NodeAddOns<Node>,
{
    fn default() -> Self {
        Self::new()
    }
}
impl<Node, AddOns> fmt::Debug for NodeHooks<Node, AddOns>
where
    Node: FullNodeComponents,
    AddOns: NodeAddOns<Node>,
{
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
    fn on_event(self: Box<Self>, node: Node) -> eyre::Result<()>;
}

impl<Node, F> OnComponentInitializedHook<Node> for F
where
    F: FnOnce(Node) -> eyre::Result<()> + Send,
{
    fn on_event(self: Box<Self>, node: Node) -> eyre::Result<()> {
        (*self)(node)
    }
}

/// A helper trait that is run once the node is started.
pub trait OnNodeStartedHook<Node: FullNodeComponents, AddOns: NodeAddOns<Node>>: Send {
    /// Consumes the event hook and runs it.
    ///
    /// If this returns an error, the node launch will be aborted.
    fn on_event(self: Box<Self>, node: FullNode<Node, AddOns>) -> eyre::Result<()>;
}

impl<Node, AddOns, F> OnNodeStartedHook<Node, AddOns> for F
where
    Node: FullNodeComponents,
    AddOns: NodeAddOns<Node>,
    F: FnOnce(FullNode<Node, AddOns>) -> eyre::Result<()> + Send,
{
    fn on_event(self: Box<Self>, node: FullNode<Node, AddOns>) -> eyre::Result<()> {
        (*self)(node)
    }
}

impl<Node> OnComponentInitializedHook<Node> for () {
    fn on_event(self: Box<Self>, _node: Node) -> eyre::Result<()> {
        Ok(())
    }
}

impl<Node, AddOns> OnNodeStartedHook<Node, AddOns> for ()
where
    Node: FullNodeComponents,
    AddOns: NodeAddOns<Node>,
{
    fn on_event(self: Box<Self>, _node: FullNode<Node, AddOns>) -> eyre::Result<()> {
        Ok(())
    }
}
