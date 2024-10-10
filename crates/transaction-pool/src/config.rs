use crate::{
    pool::{NEW_TX_LISTENER_BUFFER_SIZE, PENDING_TX_LISTENER_BUFFER_SIZE},
    PoolSize, TransactionOrigin,
};
use alloy_primitives::Address;
use reth_primitives::{
    constants::{ETHEREUM_BLOCK_GAS_LIMIT, MIN_PROTOCOL_BASE_FEE},
    EIP4844_TX_TYPE_ID,
};
use std::{collections::HashSet, ops::Mul};

/// Guarantees max transactions for one sender, compatible with geth/erigon
pub const TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER: usize = 16;

/// The default maximum allowed number of transactions in the given subpool.
pub const TXPOOL_SUBPOOL_MAX_TXS_DEFAULT: usize = 10_000;

/// The default maximum allowed size of the given subpool.
pub const TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT: usize = 20;

/// The default additional validation tasks size.
pub const DEFAULT_TXPOOL_ADDITIONAL_VALIDATION_TASKS: usize = 1;

/// Default price bump (in %) for the transaction pool underpriced check.
pub const DEFAULT_PRICE_BUMP: u128 = 10;

/// Replace blob price bump (in %) for the transaction pool underpriced check.
///
/// This enforces that a blob transaction requires a 100% price bump to be replaced
pub const REPLACE_BLOB_PRICE_BUMP: u128 = 100;

/// Configuration options for the Transaction pool.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Max number of transaction in the pending sub-pool
    pub pending_limit: SubPoolLimit,
    /// Max number of transaction in the basefee sub-pool
    pub basefee_limit: SubPoolLimit,
    /// Max number of transaction in the queued sub-pool
    pub queued_limit: SubPoolLimit,
    /// Max number of transactions in the blob sub-pool
    pub blob_limit: SubPoolLimit,
    /// Max number of executable transaction slots guaranteed per account
    pub max_account_slots: usize,
    /// Price bump (in %) for the transaction pool underpriced check.
    pub price_bumps: PriceBumpConfig,
    /// Minimum base fee required by the protocol.
    pub minimal_protocol_basefee: u64,
    /// The max gas limit for transactions in the pool
    pub gas_limit: u64,
    /// How to handle locally received transactions:
    /// [`TransactionOrigin::Local`](TransactionOrigin).
    pub local_transactions_config: LocalTransactionConfig,
    /// Bound on number of pending transactions from `reth_network::TransactionsManager` to buffer.
    pub pending_tx_listener_buffer_size: usize,
    /// Bound on number of new transactions from `reth_network::TransactionsManager` to buffer.
    pub new_tx_listener_buffer_size: usize,
}

impl PoolConfig {
    /// Returns whether the size and amount constraints in any sub-pools are exceeded.
    #[inline]
    pub const fn is_exceeded(&self, pool_size: PoolSize) -> bool {
        self.blob_limit.is_exceeded(pool_size.blob, pool_size.blob_size) ||
            self.pending_limit.is_exceeded(pool_size.pending, pool_size.pending_size) ||
            self.basefee_limit.is_exceeded(pool_size.basefee, pool_size.basefee_size) ||
            self.queued_limit.is_exceeded(pool_size.queued, pool_size.queued_size)
    }
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            pending_limit: Default::default(),
            basefee_limit: Default::default(),
            queued_limit: Default::default(),
            blob_limit: Default::default(),
            max_account_slots: TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER,
            price_bumps: Default::default(),
            minimal_protocol_basefee: MIN_PROTOCOL_BASE_FEE,
            gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
            local_transactions_config: Default::default(),
            pending_tx_listener_buffer_size: PENDING_TX_LISTENER_BUFFER_SIZE,
            new_tx_listener_buffer_size: NEW_TX_LISTENER_BUFFER_SIZE,
        }
    }
}

/// Size limits for a sub-pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubPoolLimit {
    /// Maximum amount of transaction in the pool.
    pub max_txs: usize,
    /// Maximum combined size (in bytes) of transactions in the pool.
    pub max_size: usize,
}

impl SubPoolLimit {
    /// Creates a new instance with the given limits.
    pub const fn new(max_txs: usize, max_size: usize) -> Self {
        Self { max_txs, max_size }
    }

    /// Returns whether the size or amount constraint is violated.
    #[inline]
    pub const fn is_exceeded(&self, txs: usize, size: usize) -> bool {
        self.max_txs < txs || self.max_size < size
    }
}

impl Mul<usize> for SubPoolLimit {
    type Output = Self;

    fn mul(self, rhs: usize) -> Self::Output {
        let Self { max_txs, max_size } = self;
        Self { max_txs: max_txs * rhs, max_size: max_size * rhs }
    }
}

impl Default for SubPoolLimit {
    fn default() -> Self {
        // either 10k transactions or 20MB
        Self {
            max_txs: TXPOOL_SUBPOOL_MAX_TXS_DEFAULT,
            max_size: TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT * 1024 * 1024,
        }
    }
}

/// Price bump config (in %) for the transaction pool underpriced check.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct PriceBumpConfig {
    /// Default price bump (in %) for the transaction pool underpriced check.
    pub default_price_bump: u128,
    /// Replace blob price bump (in %) for the transaction pool underpriced check.
    pub replace_blob_tx_price_bump: u128,
}

impl PriceBumpConfig {
    /// Returns the price bump required to replace the given transaction type.
    #[inline]
    pub(crate) const fn price_bump(&self, tx_type: u8) -> u128 {
        if tx_type == EIP4844_TX_TYPE_ID {
            return self.replace_blob_tx_price_bump
        }
        self.default_price_bump
    }
}

impl Default for PriceBumpConfig {
    fn default() -> Self {
        Self {
            default_price_bump: DEFAULT_PRICE_BUMP,
            replace_blob_tx_price_bump: REPLACE_BLOB_PRICE_BUMP,
        }
    }
}

/// Configuration options for the locally received transactions:
/// [`TransactionOrigin::Local`](TransactionOrigin)
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LocalTransactionConfig {
    /// Apply no exemptions to the locally received transactions.
    ///
    /// This includes:
    ///   - available slots are limited to the configured `max_account_slots` of [`PoolConfig`]
    ///   - no price exemptions
    ///   - no eviction exemptions
    pub no_exemptions: bool,
    /// Addresses that will be considered as local. Above exemptions apply.
    pub local_addresses: HashSet<Address>,
    /// Flag indicating whether local transactions should be propagated.
    pub propagate_local_transactions: bool,
}

impl Default for LocalTransactionConfig {
    fn default() -> Self {
        Self {
            no_exemptions: false,
            local_addresses: HashSet::default(),
            propagate_local_transactions: true,
        }
    }
}

impl LocalTransactionConfig {
    /// Returns whether local transactions are not exempt from the configured limits.
    #[inline]
    pub const fn no_local_exemptions(&self) -> bool {
        self.no_exemptions
    }

    /// Returns whether the local addresses vector contains the given address.
    #[inline]
    pub fn contains_local_address(&self, address: Address) -> bool {
        self.local_addresses.contains(&address)
    }

    /// Returns whether the particular transaction should be considered local.
    ///
    /// This always returns false if the local exemptions are disabled.
    #[inline]
    pub fn is_local(&self, origin: TransactionOrigin, sender: Address) -> bool {
        if self.no_local_exemptions() {
            return false
        }
        origin.is_local() || self.contains_local_address(sender)
    }

    /// Sets toggle to propagate transactions received locally by this client (e.g
    /// transactions from `eth_sendTransaction` to this nodes' RPC server)
    ///
    /// If set to false, only transactions received by network peers (via
    /// p2p) will be marked as propagated in the local transaction pool and returned on a
    /// `GetPooledTransactions` p2p request
    pub const fn set_propagate_local_transactions(mut self, propagate_local_txs: bool) -> Self {
        self.propagate_local_transactions = propagate_local_txs;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_size_sanity() {
        let pool_size = PoolSize {
            pending: 0,
            pending_size: 0,
            basefee: 0,
            basefee_size: 0,
            queued: 0,
            queued_size: 0,
            blob: 0,
            blob_size: 0,
            ..Default::default()
        };

        // the current size is zero so this should not exceed any limits
        let config = PoolConfig::default();
        assert!(!config.is_exceeded(pool_size));

        // set them to be above the limits
        let pool_size = PoolSize {
            pending: config.pending_limit.max_txs + 1,
            pending_size: config.pending_limit.max_size + 1,
            basefee: config.basefee_limit.max_txs + 1,
            basefee_size: config.basefee_limit.max_size + 1,
            queued: config.queued_limit.max_txs + 1,
            queued_size: config.queued_limit.max_size + 1,
            blob: config.blob_limit.max_txs + 1,
            blob_size: config.blob_limit.max_size + 1,
            ..Default::default()
        };

        // now this should be above the limits
        assert!(config.is_exceeded(pool_size));
    }

    #[test]
    fn test_default_config() {
        let config = LocalTransactionConfig::default();

        assert!(!config.no_exemptions);
        assert!(config.local_addresses.is_empty());
        assert!(config.propagate_local_transactions);
    }

    #[test]
    fn test_no_local_exemptions() {
        let config = LocalTransactionConfig { no_exemptions: true, ..Default::default() };
        assert!(config.no_local_exemptions());
    }

    #[test]
    fn test_contains_local_address() {
        let address = Address::new([1; 20]);
        let mut local_addresses = HashSet::default();
        local_addresses.insert(address);

        let config = LocalTransactionConfig { local_addresses, ..Default::default() };

        // Should contain the inserted address
        assert!(config.contains_local_address(address));

        // Should not contain another random address
        assert!(!config.contains_local_address(Address::new([2; 20])));
    }

    #[test]
    fn test_is_local_with_no_exemptions() {
        let address = Address::new([1; 20]);
        let config = LocalTransactionConfig {
            no_exemptions: true,
            local_addresses: HashSet::default(),
            ..Default::default()
        };

        // Should return false as no exemptions is set to true
        assert!(!config.is_local(TransactionOrigin::Local, address));
    }

    #[test]
    fn test_is_local_without_no_exemptions() {
        let address = Address::new([1; 20]);
        let mut local_addresses = HashSet::default();
        local_addresses.insert(address);

        let config =
            LocalTransactionConfig { no_exemptions: false, local_addresses, ..Default::default() };

        // Should return true as the transaction origin is local
        assert!(config.is_local(TransactionOrigin::Local, Address::new([2; 20])));
        assert!(config.is_local(TransactionOrigin::Local, address));

        // Should return true as the address is in the local_addresses set
        assert!(config.is_local(TransactionOrigin::External, address));
        // Should return false as the address is not in the local_addresses set
        assert!(!config.is_local(TransactionOrigin::External, Address::new([2; 20])));
    }

    #[test]
    fn test_set_propagate_local_transactions() {
        let config = LocalTransactionConfig::default();
        assert!(config.propagate_local_transactions);

        let new_config = config.set_propagate_local_transactions(false);
        assert!(!new_config.propagate_local_transactions);
    }

    #[test]
    fn scale_pool_limit() {
        let limit = SubPoolLimit::default();
        let double = limit * 2;
        assert_eq!(
            double,
            SubPoolLimit { max_txs: limit.max_txs * 2, max_size: limit.max_size * 2 }
        )
    }
}
