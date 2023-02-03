//! `eth` namespace handler implementation.

mod api;
pub(crate) mod error;
mod pubsub;
mod signer;

pub use api::{EthApi, EthApiSpec};
pub use pubsub::EthPubSub;
