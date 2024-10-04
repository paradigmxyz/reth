//! Transaction pool arguments

use crate::cli::config::RethTransactionPoolConfig;
use alloy_primitives::Address;
use clap::Args;
use reth_primitives::constants::{ETHEREUM_BLOCK_GAS_LIMIT, MIN_PROTOCOL_BASE_FEE};
use reth_transaction_pool::{
    blobstore::disk::DEFAULT_MAX_CACHED_BLOBS,
    pool::{NEW_TX_LISTENER_BUFFER_SIZE, PENDING_TX_LISTENER_BUFFER_SIZE},
    validate::DEFAULT_MAX_TX_INPUT_BYTES,
    LocalTransactionConfig, PoolConfig, PriceBumpConfig, SubPoolLimit, DEFAULT_PRICE_BUMP,
    DEFAULT_TXPOOL_ADDITIONAL_VALIDATION_TASKS, REPLACE_BLOB_PRICE_BUMP,
    TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER, TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT,
    TXPOOL_SUBPOOL_MAX_TXS_DEFAULT,
};
/// Parameters for debugging purposes
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(next_help_heading = "TxPool")]
pub struct TxPoolArgs {
    /// Max number of transaction in the pending sub-pool.
    #[arg(long = "txpool.pending-max-count", alias = "txpool.pending_max_count", default_value_t = TXPOOL_SUBPOOL_MAX_TXS_DEFAULT)]
    pub pending_max_count: usize,
    /// Max size of the pending sub-pool in megabytes.
    #[arg(long = "txpool.pending-max-size", alias = "txpool.pending_max_size", default_value_t = TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT)]
    pub pending_max_size: usize,

    /// Max number of transaction in the basefee sub-pool
    #[arg(long = "txpool.basefee-max-count", alias = "txpool.basefee_max_count", default_value_t = TXPOOL_SUBPOOL_MAX_TXS_DEFAULT)]
    pub basefee_max_count: usize,
    /// Max size of the basefee sub-pool in megabytes.
    #[arg(long = "txpool.basefee-max-size", alias = "txpool.basefee_max_size", default_value_t = TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT)]
    pub basefee_max_size: usize,

    /// Max number of transaction in the queued sub-pool
    #[arg(long = "txpool.queued-max-count", alias = "txpool.queued_max_count", default_value_t = TXPOOL_SUBPOOL_MAX_TXS_DEFAULT)]
    pub queued_max_count: usize,
    /// Max size of the queued sub-pool in megabytes.
    #[arg(long = "txpool.queued-max-size", alias = "txpool.queued_max_size", default_value_t = TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT)]
    pub queued_max_size: usize,

    /// Max number of executable transaction slots guaranteed per account
    #[arg(long = "txpool.max-account-slots", alias = "txpool.max_account_slots", default_value_t = TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER)]
    pub max_account_slots: usize,

    /// Price bump (in %) for the transaction pool underpriced check.
    #[arg(long = "txpool.pricebump", default_value_t = DEFAULT_PRICE_BUMP)]
    pub price_bump: u128,

    /// Minimum base fee required by the protocol.
    #[arg(long = "txpool.minimal-protocol-fee", default_value_t = MIN_PROTOCOL_BASE_FEE)]
    pub minimal_protocol_basefee: u64,

    /// The default enforced gas limit for transactions entering the pool
    #[arg(long = "txpool.gas-limit", default_value_t = ETHEREUM_BLOCK_GAS_LIMIT)]
    pub gas_limit: u64,

    /// Price bump percentage to replace an already existing blob transaction
    #[arg(long = "blobpool.pricebump", default_value_t = REPLACE_BLOB_PRICE_BUMP)]
    pub blob_transaction_price_bump: u128,

    /// Max size in bytes of a single transaction allowed to enter the pool
    #[arg(long = "txpool.max-tx-input-bytes", alias = "txpool.max_tx_input_bytes", default_value_t = DEFAULT_MAX_TX_INPUT_BYTES)]
    pub max_tx_input_bytes: usize,

    /// The maximum number of blobs to keep in the in memory blob cache.
    #[arg(long = "txpool.max-cached-entries", alias = "txpool.max_cached_entries", default_value_t = DEFAULT_MAX_CACHED_BLOBS)]
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
    /// Number of additional transaction validation tasks to spawn.
    #[arg(long = "txpool.additional-validation-tasks", alias = "txpool.additional_validation_tasks", default_value_t = DEFAULT_TXPOOL_ADDITIONAL_VALIDATION_TASKS)]
    pub additional_validation_tasks: usize,

    /// Maximum number of pending transactions from the network to buffer
    #[arg(long = "txpool.max-pending-txns", alias = "txpool.max_pending_txns", default_value_t = PENDING_TX_LISTENER_BUFFER_SIZE)]
    pub pending_tx_listener_buffer_size: usize,

    /// Maximum number of new transactions to buffer
    #[arg(long = "txpool.max-new-txns", alias = "txpool.max_new_txns", default_value_t = NEW_TX_LISTENER_BUFFER_SIZE)]
    pub new_tx_listener_buffer_size: usize,
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
            minimal_protocol_basefee: MIN_PROTOCOL_BASE_FEE,
            gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
            blob_transaction_price_bump: REPLACE_BLOB_PRICE_BUMP,
            max_tx_input_bytes: DEFAULT_MAX_TX_INPUT_BYTES,
            max_cached_entries: DEFAULT_MAX_CACHED_BLOBS,
            no_locals: false,
            locals: Default::default(),
            no_local_transactions_propagation: false,
            additional_validation_tasks: DEFAULT_TXPOOL_ADDITIONAL_VALIDATION_TASKS,
            pending_tx_listener_buffer_size: PENDING_TX_LISTENER_BUFFER_SIZE,
            new_tx_listener_buffer_size: NEW_TX_LISTENER_BUFFER_SIZE,
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
            minimal_protocol_basefee: self.minimal_protocol_basefee,
            gas_limit: self.gas_limit,
            pending_tx_listener_buffer_size: self.pending_tx_listener_buffer_size,
            new_tx_listener_buffer_size: self.new_tx_listener_buffer_size,
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
