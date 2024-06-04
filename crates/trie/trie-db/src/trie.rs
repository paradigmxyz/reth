use crate::prefix_set::PrefixSetLoader;
use alloy_rlp::{BufMut, Encodable};
use reth_db::transaction::DbTx;
use reth_execution_errors::{StateRootError};
use reth_primitives::{
    Address, BlockNumber, B256,
};
use std::ops::RangeInclusive;
use tracing::{debug};
use reth_trie::prefix_set::TriePrefixSets;
use reth_trie::{StateRoot, StateRootProgress, StorageRoot};

#[cfg(feature = "metrics")]
use reth_trie::metrics::{TrieRootMetrics, TrieType};

pub mod state_root {
    use super::*;

    /// Create a new [`StateRoot`] instance.
    pub fn from_tx<TX: DbTx>(tx: &TX) -> StateRoot<&TX, &TX> {
        StateRoot::default()
            .with_threshold(100_000)
            .with_trie_cursor_factory(tx)
            .with_hashed_cursor_factory(tx)
            .with_prefix_sets(TriePrefixSets::default())
    }

    /// Given a block number range, identifies all the accounts and storage keys that
    /// have changed.
    ///
    /// # Returns
    ///
    /// An instance of state root calculator with account and storage prefixes loaded.
    pub fn incremental_root_calculator<TX: DbTx>(
        tx: &TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<StateRoot<&TX, &TX>, StateRootError> {
        let loaded_prefix_sets = PrefixSetLoader::new(tx).load(range)?;
        Ok(StateRoot::from_tx(tx).with_prefix_sets(loaded_prefix_sets))
    }

    /// Computes the state root of the trie with the changed account and storage prefixes and
    /// existing trie nodes.
    ///
    /// # Returns
    ///
    /// The updated state root.
    pub fn incremental_root<TX: DbTx>(
        tx: &TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<B256, StateRootError> {
        debug!(target: "trie::loader", ?range, "incremental state root");
        incremental_root_calculator(tx, range)?.root()
    }

    /// Computes the state root of the trie with the changed account and storage prefixes and
    /// existing trie nodes collecting updates in the process.
    ///
    /// Ignores the threshold.
    ///
    /// # Returns
    ///
    /// The updated state root and the trie updates.
    pub fn incremental_root_with_updates<TX: DbTx>(
        tx: &TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<(B256, reth_trie::updates::TrieUpdates), StateRootError> {
        debug!(target: "trie::loader", ?range, "incremental state root");
        incremental_root_calculator(tx, range)?.root_with_updates()
    }

    /// Computes the state root of the trie with the changed account and storage prefixes and
    /// existing trie nodes collecting updates in the process.
    ///
    /// # Returns
    ///
    /// The intermediate progress of state root computation.
    pub fn incremental_root_with_progress<TX: DbTx>(
        tx: &TX,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<StateRootProgress, StateRootError> {
        debug!(target: "trie::loader", ?range, "incremental state root with progress");
        incremental_root_calculator(tx, range)?.root_with_progress()
    }
}

pub mod storage_root {
    use super::*;

    /// Create a new storage root calculator from database transaction and raw address.
    pub fn from_tx<TX: DbTx>(tx: &TX, address: Address) -> StorageRoot<&TX, &TX> {
        StorageRoot::new(
            tx,
            tx,
            address,
            #[cfg(feature = "metrics")]
                TrieRootMetrics::new(TrieType::Storage),
        )
    }

    /// Create a new storage root calculator from database transaction and hashed address.
    pub fn from_tx_hashed<TX: DbTx>(tx: &TX, hashed_address: B256) -> StorageRoot<&TX, &TX> {
        StorageRoot::new_hashed(
            tx,
            tx,
            hashed_address,
            #[cfg(feature = "metrics")]
                TrieRootMetrics::new(TrieType::Storage),
        )
    }
}
