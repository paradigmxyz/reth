use crate::utils::DbTool;
use ahash::AHasher;
use clap::Parser;
use reth_db::{
    cursor::DbCursorRO, database::Database, table::Table, transaction::DbTx, AccountChangeSets,
    AccountsHistory, AccountsTrie, BlockBodyIndices, BlockOmmers, BlockWithdrawals, Bytecodes,
    CanonicalHeaders, DatabaseEnv, HashedAccounts, HashedStorages, HeaderNumbers,
    HeaderTerminalDifficulties, Headers, PlainAccountState, PlainStorageState, PruneCheckpoints,
    RawKey, RawTable, RawValue, Receipts, StageCheckpointProgresses, StageCheckpoints,
    StorageChangeSets, StoragesHistory, StoragesTrie, TableViewer, Tables, TransactionBlocks,
    TransactionHashNumbers, TransactionSenders, Transactions, VersionHistory,
};
use std::{hash::Hasher, time::Instant};
use tracing::{info, warn};

#[derive(Parser, Debug)]
/// The arguments for the `reth db checksum` command
pub struct Command;

impl Command {
    /// Execute `db checksum` command
    pub fn execute(self, tool: &DbTool<DatabaseEnv>) -> eyre::Result<()> {
        let viewer = ChecksumViewer { tool };

        let tables = Tables::ALL;
        for table in tables {
            let checksum = match table {
                Tables::CanonicalHeaders => {
                    viewer.view::<CanonicalHeaders>()?
                }
                Tables::HeaderTerminalDifficulties => {
                    viewer.view::<HeaderTerminalDifficulties>()?
                }
                Tables::HeaderNumbers => {
                    viewer.view::<HeaderNumbers>()?
                }
                Tables::Headers => viewer.view::<Headers>()?,
                Tables::BlockBodyIndices => {
                    viewer.view::<BlockBodyIndices>()?
                }
                Tables::BlockOmmers => {
                    viewer.view::<BlockOmmers>()?
                }
                Tables::BlockWithdrawals => {
                    viewer.view::<BlockWithdrawals>()?
                }
                Tables::TransactionBlocks => {
                    viewer.view::<TransactionBlocks>()?
                }
                Tables::Transactions => {
                    viewer.view::<Transactions>()?
                }
                Tables::TransactionHashNumbers => {
                    viewer.view::<TransactionHashNumbers>()?
                }
                Tables::Receipts => viewer.view::<Receipts>()?,
                Tables::PlainAccountState => {
                    viewer.view::<PlainAccountState>()?
                }
                Tables::PlainStorageState => {
                    viewer.view::<PlainStorageState>()?
                }
                Tables::Bytecodes => viewer.view::<Bytecodes>()?,
                Tables::AccountsHistory => {
                    viewer.view::<AccountsHistory>()?
                }
                Tables::StoragesHistory => {
                    viewer.view::<StoragesHistory>()?
                }
                Tables::AccountChangeSets => {
                    viewer.view::<AccountChangeSets>()?
                }
                Tables::StorageChangeSets => {
                    viewer.view::<StorageChangeSets>()?
                }
                Tables::HashedAccounts => {
                    viewer.view::<HashedAccounts>()?
                }
                Tables::HashedStorages => {
                    viewer.view::<HashedStorages>()?
                }
                Tables::AccountsTrie => {
                    viewer.view::<AccountsTrie>()?
                }
                Tables::StoragesTrie => {
                    viewer.view::<StoragesTrie>()?
                }
                Tables::TransactionSenders => {
                    viewer.view::<TransactionSenders>()?
                }
                Tables::StageCheckpoints => {
                    viewer.view::<StageCheckpoints>()?
                }
                Tables::StageCheckpointProgresses => {
                    viewer.view::<StageCheckpointProgresses>()?
                }
                Tables::PruneCheckpoints => {
                    viewer.view::<PruneCheckpoints>()?
                }
                Tables::VersionHistory => {
                    viewer.view::<VersionHistory>()?
                }
            };
        }

        Ok(())
    }
}

struct ChecksumViewer<'a, DB: Database> {
    tool: &'a DbTool<DB>,
}

impl<DB: Database> TableViewer<()> for ChecksumViewer<'_, DB> {
    type Error = eyre::Report;

    fn view<T: Table>(&self) -> Result<(), Self::Error> {
        warn!("This command should be run without the node running!");

        let provider =
            self.tool.provider_factory.provider()?.disable_long_read_transaction_safety();
        let tx = provider.tx_ref();

        let mut cursor = tx.cursor_read::<RawTable<T>>()?;
        let walker = cursor.walk(None)?;

        let start_time = Instant::now();
        let mut hasher = AHasher::default();
        for (index, entry) in walker.enumerate() {
            let (k, v): (RawKey<T::Key>, RawValue<T::Value>) = entry?;

            if index % 100_000 == 0 {
                info!("Hashed {index} entries.");
            }

            hasher.write(k.raw_key());
            hasher.write(v.raw_value());
        }

        let elapsed = start_time.elapsed();
        let checksum = hasher.finish();
        info!("{} checksum: {:x}, took {:?}", T::NAME, checksum, elapsed);

        Ok(())
    }
}
