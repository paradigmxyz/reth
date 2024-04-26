//! Command that imports receipts from a file.

use crate::{
    args::{
        utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs,
    },
    dirs::{DataDirPath, MaybePlatformPath},
};
use clap::Parser;
use reth_db::{database::Database, init_db, DatabaseEnv};
use reth_downloaders::{
    file_client::{ChunkedFileReader, DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE},
    receipt_file_client::ReceiptFileClient,
};
use reth_node_core::version::SHORT_VERSION;
use reth_primitives::ChainSpec;
use reth_provider::{BundleStateWithReceipts, OriginalValuesKnown, ProviderFactory};
use tracing::{debug, info};

use std::{path::PathBuf, sync::Arc};

/// Initializes the database with the genesis block.
#[derive(Debug, Parser)]
pub struct ImportReceiptsCommand {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t)]
    datadir: MaybePlatformPath<DataDirPath>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,

    /// Chunk byte length.
    #[arg(long, value_name = "CHUNK_LEN", verbatim_doc_comment)]
    chunk_len: Option<u64>,

    #[command(flatten)]
    db: DatabaseArgs,

    /// The path to a receipts file for import.
    #[arg(value_name = "IMPORT_PATH", verbatim_doc_comment)]
    path: PathBuf,
}

impl ImportReceiptsCommand {
    /// Execute `import` command
    pub async fn execute(self) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

        debug!(target: "reth::cli",
            chunk_byte_len=self.chunk_len.unwrap_or(DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE), "Chunking chain import"
        );

        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);

        let db_path = data_dir.db_path();

        info!(target: "reth::cli", path = ?db_path, "Opening database");
        let db = Arc::new(init_db(db_path, self.db.database_args())?);
        info!(target: "reth::cli", "Database opened");
        let provider_factory =
            ProviderFactory::new(db.clone(), self.chain.clone(), data_dir.static_files_path())?;

        // open file
        let mut reader = ChunkedFileReader::new(&self.path, self.chunk_len).await?;

        let provider = provider_factory.provider_rw()?;

        while let Some(file_client) = reader.next_chunk::<ReceiptFileClient>().await? {
            // create a new file client from chunk read from file
            info!(target: "reth::cli",
                "Importing receipt file chunk"
            );

            let ReceiptFileClient { receipts, first_block } = file_client;

            let bundled_state =
                BundleStateWithReceipts::new(Default::default(), receipts, first_block);

            bundled_state.write_to_storage::<<DatabaseEnv as Database>::TXMut>(
                provider.tx_ref(),
                None,
                OriginalValuesKnown::Yes,
            )?;
        }

        info!(target: "reth::cli", "Receipt file imported");

        Ok(())
    }
}
