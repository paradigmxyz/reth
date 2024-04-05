//! Additional helper traits that allow for more customization.

use crate::eth::error::EthResult;
use std::fmt;

/// A trait that allows for forwarding raw transactions.
///
/// For example to a sequencer.
#[async_trait::async_trait]
pub trait RawTransactionForwarder: fmt::Debug + Send + Sync + 'static {
    /// Forwards raw transaction bytes for `eth_sendRawTransaction`
    async fn forward_raw_transaction(&self, raw: &[u8]) -> EthResult<()>;
}
