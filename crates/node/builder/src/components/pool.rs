//! Pool component for the node builder.

use alloy_primitives::Address;
use reth_transaction_pool::{PoolConfig, SubPoolLimit, TransactionPool};
use std::{collections::HashSet, future::Future};

use crate::{BuilderContext, FullNodeTypes};

/// A type that knows how to build the transaction pool.
pub trait PoolBuilder<Node: FullNodeTypes>: Send {
    /// The transaction pool to build.
    type Pool: TransactionPool + Unpin + 'static;

    /// Creates the transaction pool.
    fn build_pool(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Self::Pool>> + Send;
}

impl<Node, F, Fut, Pool> PoolBuilder<Node> for F
where
    Node: FullNodeTypes,
    Pool: TransactionPool + Unpin + 'static,
    F: FnOnce(&BuilderContext<Node>) -> Fut + Send,
    Fut: Future<Output = eyre::Result<Pool>> + Send,
{
    type Pool = Pool;

    fn build_pool(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Self::Pool>> {
        self(ctx)
    }
}

/// Convenience type to override cli or default pool configuration during build.
#[derive(Debug, Clone, Default)]
pub struct PoolBuilderConfigOverrides {
    /// Max number of transaction in the pending sub-pool
    pub pending_limit: Option<SubPoolLimit>,
    /// Max number of transaction in the basefee sub-pool
    pub basefee_limit: Option<SubPoolLimit>,
    /// Max number of transaction in the queued sub-pool
    pub queued_limit: Option<SubPoolLimit>,
    /// Max number of transactions in the blob sub-pool
    pub blob_limit: Option<SubPoolLimit>,
    /// Max number of executable transaction slots guaranteed per account
    pub max_account_slots: Option<usize>,
    /// Minimum base fee required by the protocol.
    pub minimal_protocol_basefee: Option<u64>,
    /// Addresses that will be considered as local. Above exemptions apply.
    pub local_addresses: HashSet<Address>,
}

impl PoolBuilderConfigOverrides {
    /// Applies the configured overrides to the given [`PoolConfig`].
    pub fn apply(self, mut config: PoolConfig) -> PoolConfig {
        let Self {
            pending_limit,
            basefee_limit,
            queued_limit,
            blob_limit,
            max_account_slots,
            minimal_protocol_basefee,
            local_addresses,
        } = self;

        if let Some(pending_limit) = pending_limit {
            config.pending_limit = pending_limit;
        }
        if let Some(basefee_limit) = basefee_limit {
            config.basefee_limit = basefee_limit;
        }
        if let Some(queued_limit) = queued_limit {
            config.queued_limit = queued_limit;
        }
        if let Some(blob_limit) = blob_limit {
            config.blob_limit = blob_limit;
        }
        if let Some(max_account_slots) = max_account_slots {
            config.max_account_slots = max_account_slots;
        }
        if let Some(minimal_protocol_basefee) = minimal_protocol_basefee {
            config.minimal_protocol_basefee = minimal_protocol_basefee;
        }
        config.local_transactions_config.local_addresses.extend(local_addresses);

        config
    }
}
