//! Command that initializes the node by importing a chain from a file.

use crate::{
    args::{
        utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs,
    },
    dirs::{DataDirPath, MaybePlatformPath},
    primitives::{address, bloom, Address, Bytes, TxHash, U256},
    version::SHORT_VERSION,
};
use alloy_rlp::RlpDecodable;
use clap::Parser;
use reth_db::{
    init_db, tables,
    transaction::{DbTx, DbTxMut},
};
use reth_node_core::init::init_genesis;
use reth_primitives::{
    b256, fs,
    stage::{StageCheckpoint, StageId},
    ChainSpec, Genesis, Header, SealedBlock, SealedBlockWithSenders, B256,
};
use reth_provider::{BlockWriter, ProviderFactory};
use reth_revm::primitives::FixedBytes;
use reth_stages::prelude::*;
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};
use tracing::{debug, info};

/// Syncs RLP encoded blocks from a file.
#[derive(Debug, Parser)]
pub struct ImportOpCommand {
    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment)]
    config: Option<PathBuf>,

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

    #[command(flatten)]
    db: DatabaseArgs,

    /// Path to file with state at first bedrock block.
    #[arg(value_name = "IMPORT_PATH", verbatim_doc_comment)]
    path: PathBuf,
}

impl ImportOpCommand {
    /// Execute `import` command
    pub async fn execute(self) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

        info!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Loading genesis from file");
        let genesis_json: Genesis = serde_json::from_str(include_str!(
            "../../../../crates/primitives/res/genesis/optimism.json"
        ))
        .expect("Can't deserialize Optimism Mainnet genesis json");

        let chain = ChainSpec::from(genesis_json);

        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(chain.chain);

        let db_path = data_dir.db_path();

        info!(target: "reth::cli", path = ?db_path, "Opening database");
        let db = Arc::new(init_db(db_path, self.db.database_args())?);
        info!(target: "reth::cli", "Database opened");

        let provider_factory =
            ProviderFactory::new(db, self.chain.clone(), data_dir.static_files_path())?;

        debug!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Initializing genesis");

        init_genesis(provider_factory.clone())?;

        let provider = provider_factory.provider_rw()?;
        // Fill in fake data for all tables with block as key up until block 105235062 inclusive.
        // Dummy blocks don't have transactions, so there are no receipts to insert.
        for block_number in 0..=105235062 {
            debug!(target: "reth::cli", "inserting dummy block {}", block_number);
            let sealed_header =
                Header { number: block_number, ..Default::default() }.seal(Default::default());

            let block: SealedBlockWithSenders = SealedBlockWithSenders {
                block: SealedBlock { header: sealed_header, ..Default::default() },
                ..Default::default()
            };
            provider.insert_block(block, None)?;
        }

        let sealed_header = Header {
            parent_hash: b256!("21a168dfa5e727926063a28ba16fd5ee84c814e847c81a699c7a0ea551e4ca50"),
            base_fee_per_gas: Some(1000000000),
            difficulty: U256::from(0),
            extra_data: Bytes::from("BEDROCK"),
            gas_limit: 30000000,
            gas_used: 0,
            logs_bloom: bloom!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"),
            beneficiary: address!("4200000000000000000000000000000000000011"),
            mix_hash: b256!("0000000000000000000000000000000000000000000000000000000000000000"),
            nonce: 0,
            number: 105235063,
            receipts_root: b256!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"),
            ommers_hash: b256!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"),
            state_root: b256!("bfe2b059bc76c33556870c292048f1d28c9d498462a02a3c7aadb6edf1c2d21c"),
            timestamp: 1673550516,
            transactions_root: b256!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"),
            blob_gas_used: None,
            excess_blob_gas: None,
            withdrawals_root: None,
            parent_beacon_block_root: None,
        }.seal_slow();

        let block: SealedBlockWithSenders = SealedBlockWithSenders {
            block: SealedBlock {
                header: sealed_header,
                body: vec![],
                withdrawals: None,
                ommers: vec![],
            },
            senders: vec![],
        };
        dbg!(block.hash());
        provider.insert_block(block, None)?;
        let tx = provider.into_tx();

        let stage_checkpoint = StageCheckpoint { block_number: 105235063, stage_checkpoint: None };

        info!(target: "reth::cli", "commiting sync stages");
        for stage in StageId::ALL.iter() {
            tx.put::<tables::StageCheckpoints>(stage.to_string(), stage_checkpoint)?;
        }

        tx.commit()?;

        Ok(())
    }
}
