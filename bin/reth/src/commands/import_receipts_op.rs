//! Command that imports receipts from a file.

use crate::{
    args::{
        utils::{genesis_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs,
    },
    commands::import_receipts::import_receipts_from_file,
    dirs::{DataDirPath, MaybePlatformPath},
};
use clap::Parser;
use reth_db::init_db;
use reth_downloaders::file_client::DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE;
use reth_node_core::version::SHORT_VERSION;
use reth_optimism_primitives::bedrock_import::is_dup_tx;
use reth_primitives::Receipts;
use reth_provider::ProviderFactory;
use tracing::{debug, info};

use std::{path::PathBuf, sync::Arc};

/// Initializes the database with the genesis block.
#[derive(Debug, Parser)]
pub struct ImportReceiptsOpCommand {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t)]
    datadir: MaybePlatformPath<DataDirPath>,

    /// Chunk byte length.
    #[arg(long, value_name = "CHUNK_LEN", verbatim_doc_comment)]
    chunk_len: Option<u64>,

    #[command(flatten)]
    db: DatabaseArgs,

    /// The path to a receipts file for import. File must use `HackReceiptFileCodec` (used for
    /// exporting OP chain segment below Bedrock block via testinprod/op-geth).
    ///
    /// <https://github.com/testinprod-io/op-geth/pull/1>
    #[arg(value_name = "IMPORT_PATH", verbatim_doc_comment)]
    path: PathBuf,
}

impl ImportReceiptsOpCommand {
    /// Execute `import` command
    pub async fn execute(self) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

        debug!(target: "reth::cli",
            chunk_byte_len=self.chunk_len.unwrap_or(DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE),
            "Chunking receipts import"
        );

        let chain_spec = genesis_value_parser(SUPPORTED_CHAINS[0])?;

        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(chain_spec.chain);

        let db_path = data_dir.db();
        info!(target: "reth::cli", path = ?db_path, "Opening database");

        let db = Arc::new(init_db(db_path, self.db.database_args())?);
        info!(target: "reth::cli", "Database opened");
        let provider_factory =
            ProviderFactory::new(db.clone(), chain_spec.clone(), data_dir.static_files())?;

        import_receipts_from_file(
            provider_factory,
            self.path,
            self.chunk_len,
            |first_block, receipts: &mut Receipts| {
                let mut total_filtered_out_dup_txns = 0;
                for (index, receipts_for_block) in receipts.iter_mut().enumerate() {
                    if is_dup_tx(first_block + index as u64) {
                        receipts_for_block.clear();
                        total_filtered_out_dup_txns += 1;
                    }
                }

                total_filtered_out_dup_txns
            },
        )
        .await
    }
}
