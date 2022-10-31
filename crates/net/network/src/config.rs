use std::sync::Arc;

/// All network related initialization settings.
pub struct NetworkConfig<C> {
    /// The client type that can interact with the chain.
    client: Arc<C>,
    /// The node's secret key, from which the node's identity is derived.
    secret_key: (),
    // TODO config for boot nodes etc.
}
