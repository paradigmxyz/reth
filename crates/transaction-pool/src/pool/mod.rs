//! Transaction Pool internals.
//!
//! Incoming transactions are validated first. The validation outcome can have 3 states:
//!     1. Transaction can _never_ be valid
//!     2. Transaction is _currently_ valid
//!     3. Transaction is _currently_ invalid, but could potentially become valid in the future
//!
//! However, (2.) and (3.) of a transaction can only be determined on the basis of the current
//! state, whereas (1.) holds indefinitely. This means once the state changes (2.) and (3.) need to
//! be reevaluated again.
//!
//! The transaction pool is responsible for storing new, valid transactions and providing the next
//! best transactions sorted by their priority. Where priority is determined by the transaction's
//! score.
//!
//! However, the score is also only valid for the current state.
//!
//! In essence the transaction pool is made of two separate sub-pools for currently valid (2.) and
//! currently invalid (3.).
//!
//! Depending on the use case, consumers of the [`TransactionPool`](crate::traits::TransactionPool)
//! are interested in (2.) and/or (3.).

//! A generic [`TransactionPool`](crate::traits::TransactionPool) that only handles transactions.
//!
//! This Pool maintains two separate sub-pools for (2.) and (3.)
//!
//! ## Terminology
//!
//!     - _Pending_: pending transactions are transactions that fall under (2.). Those transactions
//!       are _currently_ ready to be executed and are stored in the `pending` sub-pool
//!     - _Queued_: queued transactions are transactions that fall under category (3.). Those
//!       transactions are _currently_ waiting for state changes that eventually move them into
//!       category (2.) and become pending.
use crate::{pool::listener::PoolEventListener, PoolClient, PoolConfig};
use parking_lot::RwLock;
use std::sync::Arc;

mod events;
mod listener;
mod pending;
mod queued;

// TODO find better name
pub struct Pool<PoolApi: PoolClient> {
    /// Chain/Storage access
    client: Arc<PoolApi>,
    /// Pool settings
    config: PoolConfig,
    /// Listeners for transaction state change events
    listeners: RwLock<PoolEventListener<PoolApi::Hash, PoolApi::BlockHash>>, /* TODO needs the actual sub-pools
                                                                              * TODO needs listeners for incoming transactions */
}
