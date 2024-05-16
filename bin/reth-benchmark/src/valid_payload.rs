//! This is a future that waits until we get a VALID response from the given engine API provider.

use alloy_provider::{Provider, ProviderBuilder};

/// This contains an authenticated provider that can be used to send engine API newPayload requests.
///
/// The waiter is initialized with an authenticated provider and an execution payload.
pub struct ValidPayloadWaiter {
    // todo
}
