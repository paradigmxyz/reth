//! `eth` namespace handler implementation.

mod api;
mod pubsub;

pub use api::{EthApi, EthApiSpec};
pub use pubsub::EthPubSub;
