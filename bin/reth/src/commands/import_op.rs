//! Command that initializes the node by importing a chain from a file.

use crate::{
    args::{
        utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs,
    },
    dirs::{DataDirPath, MaybePlatformPath},
    primitives::{
        Address, Bytes, TxHash, U256,
    },
    version::SHORT_VERSION,
};
use alloy_rlp::{RlpDecodable};
use clap::Parser;
use reth_db::{init_db, tables};
use reth_node_core::{init::init_genesis};
use reth_primitives::{stage::StageId, ChainSpec, B256, SealedBlockWithSenders, SealedBlock, Header, fs, Genesis, b256, Bloom};
use reth_provider::{BlockWriter, ProviderFactory};
use reth_stages::{
    prelude::*,
};
use serde::Deserialize;
use std::{
    path::{PathBuf},
    sync::Arc,
};
use std::io::Read;
use crossterm::terminal::size;
use tracing::{debug, info};
use reth_db::transaction::{DbTx, DbTxMut};
use reth_revm::primitives::FixedBytes;
use reth_primitives::stage::StageCheckpoint;
use crate::primitives::{address, bloom};

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

    /// The path to a block file for import.
    ///
    /// The online stages (headers and bodies) are replaced by a file import, after which the
    /// remaining stages are executed.
    #[arg(value_name = "IMPORT_PATH", verbatim_doc_comment)]
    path: PathBuf,
}

/// Ethereum full block.
#[derive(Debug, Clone, PartialEq, Eq, RlpDecodable)]
pub struct Block {
    pub header: RlpHeader,
    pub txs: Vec<Transaction>,
    pub uncles: Vec<RlpHeader>,
}

// Block header
#[derive(Debug, Clone, PartialEq, Eq, RlpDecodable)]
#[rlp(trailing)]
pub struct RlpHeader {
    /// The Keccak 256-bit hash of the parent
    /// block’s header, in its entirety; formally Hp.
    pub parent_hash: B256,
    /// The Keccak 256-bit hash of the ommers list portion of this block; formally Ho.
    pub uncle_hash: B256,
    /// The 160-bit address to which all fees collected from the successful mining of this block
    /// be transferred; formally Hc.
    pub coinbase: Address,
    /// The Keccak 256-bit hash of the root node of the state trie, after all transactions are
    /// executed and finalisations applied; formally Hr.
    pub root: B256,
    /// The Keccak 256-bit hash of the root node of the trie structure populated with each
    /// transaction in the transactions list portion of the block; formally Ht.
    pub tx_hash: B256,
    /// The Keccak 256-bit hash of the root node of the trie structure populated with the receipts
    /// of each transaction in the transactions list portion of the block; formally He.
    pub receipt_hash: B256,
    /// The Bloom filter composed from indexable information (logger address and log topics)
    /// contained in each log entry from the receipt of each transaction in the transactions list;
    /// formally Hb.
    pub bloom: FixedBytes<256>,
    /// A scalar value corresponding to the difficulty level of this block. This can be calculated
    /// from the previous block’s difficulty level and the timestamp; formally Hd.
    pub difficulty: U256,
    /// A scalar value equal to the number of ancestor blocks. The genesis block has a number of
    /// zero; formally Hi.
    pub number: U256,
    /// A scalar value equal to the current limit of gas expenditure per block; formally Hl.
    pub gas_limit: u64,
    /// A scalar value equal to the total gas used in transactions in this block; formally Hg.
    pub gas_used: u64,
    /// A scalar value equal to the reasonable output of Unix’s time() at this block’s inception;
    /// formally Hs.
    pub time: u64,
    /// An arbitrary byte array containing data relevant to this block. This must be 32 bytes or
    /// fewer; formally Hx.
    pub extra_data: Bytes,
    /// A 256-bit hash which, combined with the
    /// nonce, proves that a sufficient amount of computation has been carried out on this block;
    /// formally Hm.
    pub mix_digest: Bytes,
    /// A 64-bit value which, combined with the mixhash, proves that a sufficient amount of
    /// computation has been carried out on this block; formally Hn.
    pub nonce: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, RlpDecodable)]
pub struct Transaction {
    pub data: TxLegacy,
    pub meta: TxMeta,
    /// Transaction hash
    pub hash: TxHash,
    pub size: u32,
    pub from: Address,
}

#[derive(Eq, PartialEq, Deserialize, Clone, Debug, RlpDecodable)]
pub struct TxMeta {
    block_number: U256,
    timestamp: u64,
    message_sender: Address,
    rest: Bytes,
}

#[derive(Eq, PartialEq, Deserialize, Clone, Debug, RlpDecodable)]
pub struct TxLegacy {
    /// A scalar value equal to the number of transactions sent by the sender; formally Tn.
    pub account_nonce: u64,
    /// A scalar value equal to the number of
    /// Wei to be paid per unit of gas for all computation
    /// costs incurred as a result of the execution of this transaction; formally Tp.
    ///
    /// As ethereum circulation is around 120mil eth as of 2022 that is around
    /// 120000000000000000000000000 wei we are safe to use u128 as its max number is:
    /// 340282366920938463463374607431768211455
    pub gas_price: U256,
    /// A scalar value equal to the maximum
    /// amount of gas that should be used in executing
    /// this transaction. This is paid up-front, before any
    /// computation is done and may not be increased
    /// later; formally Tg.
    pub gas_limit: u64,
    /// The 160-bit address of the message call’s recipient or, for a contract creation
    /// transaction, ∅, used here to denote the only member of B0 ; formally Tt.
    pub recipient: Address,
    /// A scalar value equal to the number of Wei to
    /// be transferred to the message call’s recipient or,
    /// in the case of contract creation, as an endowment
    /// to the newly created account; formally Tv.
    pub value: U256,
    /// Input has two uses depending if transaction is Create or Call (if `to` field is None or
    /// Some). pub init: An unlimited size byte array specifying the
    /// EVM-code for the account initialisation procedure CREATE,
    /// data: An unlimited size byte array specifying the
    /// input data of the message call, formally Td.
    pub input: Bytes,
    pub v: U256,
    pub r: U256,
    pub s: U256,
}

impl ImportOpCommand {
    /// Execute `import` command
    pub async fn execute(self) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

        debug!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Loading genesis from file");
        let genesis_json: Genesis = serde_json::from_str::<Genesis>(
            fs::read_to_string("/Users/vacekj/Programming/reth/crates/primitives/res/genesis/goerli_op.json").unwrap().as_str()).unwrap();
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
        /* Fill in fake data for all tables up until block 4061223 inclusive */
        for block_number in 0..=4061223 {
            debug!(target: "reth::cli", "inserting dummy block {}", block_number);
            let sealed_header = Header {
                number: block_number,
                ..Default::default()
            }.seal(Default::default());
        
            let block: SealedBlockWithSenders = SealedBlockWithSenders {
                block: SealedBlock {
                    header: sealed_header,
                    ..Default::default()
                },
                ..Default::default()
            };
            provider.insert_block(block, None)?;
        }

        let sealed_header = Header {
            parent_hash: b256!("31267a44f1422f4cab59b076548c075e79bd59e691a23fbce027f572a2a49dc9"),
            base_fee_per_gas: Some(1000000000),
            difficulty: U256::from(0),
            extra_data: Bytes::from("424544524f434b"),
            gas_limit: 25000000,
            gas_used: 0,
            logs_bloom: bloom!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"),
            beneficiary: address!("4200000000000000000000000000000000000011"),
            mix_hash: b256!("0000000000000000000000000000000000000000000000000000000000000000"),
            nonce: 0,
            number: 4061224,
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
            senders: vec![]
        };
        provider.insert_block(block, None)?;

        let tx = provider.into_tx();

        let stage_checkpoint = StageCheckpoint {
            block_number: 4061224,
            stage_checkpoint: None,
        };

        info!(target: "reth::cli", "commiting sync stages");
        for stage in StageId::ALL.iter() {
            tx.put::<tables::StageCheckpoints>(stage.to_string(), stage_checkpoint)?;
        }

        tx.commit()?;

        Ok(())
    }
}
