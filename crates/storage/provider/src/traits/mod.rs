//! Collection of common provider traits.

// Re-export all the traits
pub use reth_storage_api::*;

pub use reth_chainspec::ChainSpecProvider;

mod static_file_provider;
pub use static_file_provider::StaticFileProviderFactory;

mod full;
pub use full::FullProvider;

/// Trait for providers that support configurable transaction sender storage
pub trait SenderRecoveryProvider {
    /// Returns whether transaction senders should be stored in static files
    fn static_file_senders(&self) -> bool;
}
