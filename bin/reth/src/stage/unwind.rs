//! Unwinding a certain block range

use crate::dirs::{DataDirPath, MaybePlatformPath};
use clap::{Parser, Subcommand};
use reth_db::{
    database::Database,
    mdbx::{Env, WriteMap},
    tables,
    transaction::DbTx,
};
use reth_primitives::{BlockHashOrNumber, ChainSpec};
use reth_provider::Transaction;
use reth_staged_sync::utils::chainspec::genesis_value_parser;
use std::{ops::RangeInclusive, sync::Arc};

use reth_db::cursor::DbCursorRO;

/// `reth stage unwind` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t, global = true)]
    datadir: MaybePlatformPath<DataDirPath>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    ///
    /// Built-in chains:
    /// - mainnet
    /// - goerli
    /// - sepolia
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        verbatim_doc_comment,
        default_value = "mainnet",
        value_parser = genesis_value_parser,
        global = true
    )]
    chain: Arc<ChainSpec>,

    #[clap(subcommand)]
    command: Subcommands,
}

impl Command {
    /// Execute `db stage unwind` command
    pub async fn execute(self) -> eyre::Result<()> {
        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        if !db_path.exists() {
            eyre::bail!("Database {db_path:?} does not exist.")
        }

        let db = Env::<WriteMap>::open(db_path.as_ref(), reth_db::mdbx::EnvKind::RW)?;

        let range = self.command.unwind_range(&db)?;

        if *range.start() == 0 {
            eyre::bail!("Cannot unwind genesis block")
        }

        let mut tx = Transaction::new(&db)?;

        let blocks_and_execution = tx
            .take_block_and_execution_range(&self.chain, range)
            .map_err(|err| eyre::eyre!("Transaction error on unwind: {err:?}"))?;

        tx.commit()?;

        println!("Unwound {} blocks", blocks_and_execution.len());

        Ok(())
    }
}

/// `reth stage unwind` subcommand
#[derive(Subcommand, Debug, Eq, PartialEq)]
enum Subcommands {
    /// Unwinds the database until the given block number (range is inclusive).
    #[clap(name = "to-block")]
    ToBlock { target: BlockHashOrNumber },
    /// Unwinds the given number of blocks from the database.
    #[clap(name = "num-blocks")]
    NumBlocks { amount: u64 },
}

impl Subcommands {
    /// Returns the block range to unwind.
    ///
    /// This returns an inclusive range: [target..=latest]
    fn unwind_range<DB: Database>(&self, db: DB) -> eyre::Result<RangeInclusive<u64>> {
        let tx = db.tx()?;
        let mut cursor = tx.cursor_read::<tables::CanonicalHeaders>()?;
        let last = cursor.last()?.ok_or_else(|| eyre::eyre!("No blocks in database"))?;

        let target = match self {
            Subcommands::ToBlock { target } => match target {
                BlockHashOrNumber::Hash(hash) => tx
                    .get::<tables::HeaderNumbers>(*hash)?
                    .ok_or_else(|| eyre::eyre!("Block hash not found in database: {hash:?}"))?,
                BlockHashOrNumber::Number(num) => *num,
            },
            Subcommands::NumBlocks { amount } => last.0.saturating_sub(*amount) + 1,
        };
        Ok(target..=last.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_unwind() {
        let cmd = Command::parse_from(["reth", "--datadir", "dir", "to-block", "100"]);
        assert_eq!(cmd.command, Subcommands::ToBlock { target: BlockHashOrNumber::Number(100) });

        let cmd = Command::parse_from(["reth", "--datadir", "dir", "num-blocks", "100"]);
        assert_eq!(cmd.command, Subcommands::NumBlocks { amount: 100 });
    }
}
