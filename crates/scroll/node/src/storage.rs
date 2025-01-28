use reth_provider::EthStorage;
use reth_scroll_primitives::ScrollTransactionSigned;

/// The storage implementation for Scroll.
pub type ScrollStorage = EthStorage<ScrollTransactionSigned>;
