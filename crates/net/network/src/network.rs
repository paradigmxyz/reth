use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

/// A _shareable_ network frontend. Used to interact with the network.
///
/// See also [`NetworkManager`](crate::NetworkManager).
#[derive(Clone)]
pub struct NetworkHandle {
    inner: Arc<NetworkInner>,
}

struct NetworkInner {
    /// Sender half of the message channel to the [`NetworkManager`].
    to_manager_tx: UnboundedSender<NetworkHandleMessage>,
}

/// Internal messages that can be passed to the  [`NetworkManager`](crate::NetworkManager).
pub(crate) enum NetworkHandleMessage {}
