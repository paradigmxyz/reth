//! Transaction pool arguments

use crate::cli::config::RethTransactionPoolConfig;
use clap::Args;
use reth_primitives::Address;
use reth_transaction_pool::{
    blobstore::disk::DEFAULT_MAX_CACHED_BLOBS, validate::DEFAULT_MAX_TX_INPUT_BYTES,
    LocalTransactionConfig, PoolConfig, PriceBumpConfig, SubPoolLimit, DEFAULT_PRICE_BUMP,
    REPLACE_BLOB_PRICE_BUMP, TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER,
    TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT, TXPOOL_SUBPOOL_MAX_TXS_DEFAULT,
};
/// Parameters for debugging purposes
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(next_help_heading = "TxPool")]
pub struct TxPoolArgs {
    /// Max number of transaction in the pending sub-pool.
    #[arg(long = "txpool.pending_max_count", default_value_t = TXPOOL_SUBPOOL_MAX_TXS_DEFAULT)]
    pub pending_max_count: usize,
    /// Max size of the pending sub-pool in megabytes.
    #[arg(long = "txpool.pending_max_size", default_value_t = TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT)]
    pub pending_max_size: usize,

    /// Max number of transaction in the basefee sub-pool
    #[arg(long = "txpool.basefee_max_count", default_value_t = TXPOOL_SUBPOOL_MAX_TXS_DEFAULT)]
    pub basefee_max_count: usize,
    /// Max size of the basefee sub-pool in megabytes.
    #[arg(long = "txpool.basefee_max_size", default_value_t = TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT)]
    pub basefee_max_size: usize,

    /// Max number of transaction in the queued sub-pool
    #[arg(long = "txpool.queued_max_count", default_value_t = TXPOOL_SUBPOOL_MAX_TXS_DEFAULT)]
    pub queued_max_count: usize,
    /// Max size of the queued sub-pool in megabytes.
    #[arg(long = "txpool.queued_max_size", default_value_t = TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT)]
    pub queued_max_size: usize,

    /// Max number of executable transaction slots guaranteed per account
    #[arg(long = "txpool.max_account_slots", default_value_t = TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER)]
    pub max_account_slots: usize,

    /// Price bump (in %) for the transaction pool underpriced check.
    #[arg(long = "txpool.pricebump", default_value_t = DEFAULT_PRICE_BUMP)]
    pub price_bump: u128,

    /// Price bump percentage to replace an already existing blob transaction
    #[arg(long = "blobpool.pricebump", default_value_t = REPLACE_BLOB_PRICE_BUMP)]
    pub blob_transaction_price_bump: u128,

    /// Max size in bytes of a single transaction allowed to enter the pool
    #[arg(long = "txpool.max_tx_input_bytes", default_value_t = DEFAULT_MAX_TX_INPUT_BYTES)]
    pub max_tx_input_bytes: usize,

    /// The maximum number of blobs to keep in the in memory blob cache.
    #[arg(long = "txpool.max_cached_entries", default_value_t = DEFAULT_MAX_CACHED_BLOBS)]
    pub max_cached_entries: u32,

    /// Flag to disable local transaction exemptions.
    #[arg(long = "txpool.nolocals")]
    pub no_locals: bool,
    /// Flag to allow certain addresses as local.
    #[arg(long = "txpool.locals")]
    pub locals: Vec<Address>,
    /// Flag to toggle local transaction propagation.
    #[arg(long = "txpool.no-local-transactions-propagation")]
    pub no_local_transactions_propagation: bool,
}

impl Default for TxPoolArgs {
    fn default() -> Self {
        Self {
            pending_max_count: TXPOOL_SUBPOOL_MAX_TXS_DEFAULT,
            pending_max_size: TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT,
            basefee_max_count: TXPOOL_SUBPOOL_MAX_TXS_DEFAULT,
            basefee_max_size: TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT,
            queued_max_count: TXPOOL_SUBPOOL_MAX_TXS_DEFAULT,
            queued_max_size: TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT,
            max_account_slots: TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER,
            price_bump: DEFAULT_PRICE_BUMP,
            blob_transaction_price_bump: REPLACE_BLOB_PRICE_BUMP,
            max_tx_input_bytes: DEFAULT_MAX_TX_INPUT_BYTES,
            max_cached_entries: DEFAULT_MAX_CACHED_BLOBS,
            no_locals: false,
            locals: Default::default(),
            no_local_transactions_propagation: false,
        }
    }
}

impl RethTransactionPoolConfig for TxPoolArgs {
    /// Returns transaction pool configuration.
    fn pool_config(&self) -> PoolConfig {
        PoolConfig {
            local_transactions_config: LocalTransactionConfig {
                no_exemptions: self.no_locals,
                local_addresses: self.locals.clone().into_iter().collect(),
                propagate_local_transactions: !self.no_local_transactions_propagation,
            },
            pending_limit: SubPoolLimit {
                max_txs: self.pending_max_count,
                max_size: self.pending_max_size * 1024 * 1024,
            },
            basefee_limit: SubPoolLimit {
                max_txs: self.basefee_max_count,
                max_size: self.basefee_max_size * 1024 * 1024,
            },
            queued_limit: SubPoolLimit {
                max_txs: self.queued_max_count,
                max_size: self.queued_max_size * 1024 * 1024,
            },
            blob_limit: SubPoolLimit {
                max_txs: self.queued_max_count,
                max_size: self.queued_max_size * 1024 * 1024,
            },
            max_account_slots: self.max_account_slots,
            price_bumps: PriceBumpConfig {
                default_price_bump: self.price_bump,
                replace_blob_tx_price_bump: self.blob_transaction_price_bump,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    #[test]
    fn txpool_args_default_sanity_test() {
        let default_args = TxPoolArgs::default();
        let args = CommandParser::<TxPoolArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }
}
