//! A stage for indexing the addresses that appear in historical transactions.

use async_trait::async_trait;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    models::ShardedKey,
    table,
    transaction::DbTxMut,
};
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    Address, IntegerList,
};
use reth_provider::{BlockReader, DatabaseProviderRW, ProviderError};
use reth_stages::{
    stages::NonCoreStage, ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput,
};
use serde::{Deserialize, Serialize};

// === Stage ===
// For setting up a stage.

/// A stage for indexing addresses that appear.
#[derive(Debug)]
pub struct AddressStage {
    /// Threshold block number to commit to db after.
    pub commit_threshold: u64,
}

impl<DB: Database> NonCoreStage<DB> for AddressStage {
    type Config = AddressAppearancesConfig;

    fn new() -> Self {
        Self {commit_threshold: Self::Config::default().commit_threshold}
    }
}

#[async_trait]
impl<DB: Database> Stage<DB> for AddressStage {
    /// Get the ID of the stage.
    ///
    /// Stage IDs must be unique.
    fn id(&self) -> StageId {
        StageId::Other("AddressAppearances")
    }

    /// Execute the stage.
    async fn execute(
        &mut self,
        provider: &DatabaseProviderRW<'_, &DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        /*
        Algorithm:
        - Equivalent of debug_traceTransaction with CallTracer
        - Repeat for every block in batch
        - Filter for addresses, group together
        - Write to addresses table.
        */

        let tx = provider.tx_ref();
        let block_number = input.next_block();

        // todo: for all blocks in batch
        let first_tx_num = provider
            .block_body_indices(block_number)?
            .ok_or_else(|| ProviderError::BlockNotFound(block_number.into()))?
            .first_tx_num;

        let block = provider
            .block_with_senders(block_number)?
            .ok_or_else(|| ProviderError::BlockNotFound(block_number.into()))?;

        let mut appearances = vec![];
        // todo: TraceCall

        let encoded_locations = vec![first_tx_num]; // todo: encode the tx_num upper bits for the block
        appearances.push((block.beneficiary, encoded_locations));

        let mut addresses_read_cursor = tx.cursor_write::<AddressAppearances>()?;
        let mut addresses_write_cursor = tx.cursor_write::<AddressAppearances>()?;

        for (address, txs) in appearances {
            // todo: Store as encoded Tx and use ShardedKey.
            let txs_usize: Vec<usize> = txs.iter().map(|x| *x as usize).collect();
            let locations = IntegerList::new(txs_usize).expect("Could not create integerlist");

            // todo: Address sharded key already has data and extend otherwise make new entry.
            let key = ShardedKey::new(address, block_number);
            let _ = match addresses_read_cursor.seek_exact(key.clone())? {
                Some((_address, existing)) => {
                    // todo: Do this more efficiently.
                    let mut combined_vec: Vec<usize> = existing.iter(0).collect();
                    for tx in txs {
                        combined_vec.push(tx as usize);
                    }
                    let int_list =
                        IntegerList::new(combined_vec).expect("Could not create integerlist");
                    addresses_write_cursor.upsert(key, int_list)
                }
                None => addresses_write_cursor.upsert(key, locations),
            };
        }
        let is_done = true; // todo
        Ok(ExecOutput { checkpoint: StageCheckpoint::new(block_number), done: is_done })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        _provider: &DatabaseProviderRW<'_, &DB>,
        _input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        todo!()
    }
}

// === Table ===
// For setting up a database table

table!(
    /// Stores pointers to transactions for a particular address.
    ///
    /// Last shard key of the storage will contain `u64::MAX` `BlockNumber`,
    /// this would allows us small optimization on db access when change is in plain state.
    ///
    /// Imagine having shards as:
    /// * `Address | 100`
    /// * `Address | u64::MAX`
    ///
    /// What we need to find is number that is one greater than N. Db `seek` function allows us to fetch
    /// the shard that equal or more than asked. For example:
    /// * For N=50 we would get first shard.
    /// * for N=150 we would get second shard.
    /// * If max block number is 200 and we ask for N=250 we would fetch last shard and
    ///     know that needed entry is in `AccountPlainState`.
    /// * If there were no shard we would get `None` entry or entry of different storage key.
    ///
    /// Code example can be found in `reth_provider::HistoricalStateProviderRef`
    ( AddressAppearances ) ShardedKey<Address> | IntegerList
);

// === Config ===
// For setting up a stage.

/// Address appearance stage configuration.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct AddressAppearancesConfig {
    /// The maximum number of blocks to process before committing progress to the database.
    pub commit_threshold: u64,
}

impl Default for AddressAppearancesConfig {
    fn default() -> Self {
        Self { commit_threshold: 100_000 }
    }
}
