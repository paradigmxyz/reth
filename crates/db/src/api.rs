#![allow(missing_docs)]

//! Module with the API traits that memdb, libmdbx and static have to provide.
use reth_primitives::{
    Account, Address, Block, BlockHash, BlockNumber, Bytes, Header, Receipt, Transaction, TxHash,
    TxNumber, H256, U256,
};

/// Enum for API Errors.
#[derive(Debug)]
pub enum ApiError {}

/// Common API for querying and writing data.
#[rustfmt::skip]
pub trait DataApi {
    /// Returns the lowest and highest block number the data structure contains.
    fn read_block_number_range(&self) -> Result<(BlockNumber, BlockNumber), ApiError>;
    fn read_block_latest(&self) -> Result<Block, ApiError>;
    fn read_block(&self, number: Option<BlockNumber>, hash: Option<BlockHash>) -> Result<Block, ApiError>;
    fn write_block(&self, block: Block) -> Result<(), ApiError>;
    fn delete_blocks(&self, number: Option<Vec<BlockNumber>>, hash: Option<Vec<BlockHash>>) -> Result<(), ApiError>;
    fn truncate_blocks(&self, from: BlockNumber) -> Result<(), ApiError>;
    fn has_block(&self, hash: BlockHash) -> Result<bool, ApiError>;

    fn read_header_latest(&self) -> Result<Header, ApiError>;
    fn read_header(&self, number: Option<u64>, hash: H256) -> Result<Header, ApiError>;
    fn read_header_number(&self, hash: H256) -> Result<u64, ApiError>;
    fn write_header(&self, header: Header) -> Result<(), ApiError>;
    fn write_header_number(&self, hash: H256) -> Result<(), ApiError>;
    fn delete_headers(&self, header: Vec<Header>) -> Result<(), ApiError>;

    fn read_tx( &self, number: Option<TxNumber>, hash: Option<TxHash>) -> Result<Transaction, ApiError>;
    fn write_tx(&self, transaction: Transaction) -> Result<(), ApiError>;
    fn delete_txes(&self, number: Option<Vec<TxNumber>>, hash: Option<Vec<TxHash>>) -> Result<(), ApiError>;
    fn write_tx_senders(&self, senders: Vec<(TxHash, Address)>) -> Result<(), ApiError>;
    fn move_to_canonical_txes(&self) -> Result<(), ApiError>;
    fn move_to_non_canonical_txes(&self) -> Result<(), ApiError>;

    fn read_canonical_hash(&self, number: u64) -> Result<H256, ApiError>;
    fn write_total_difficulty(&self, difficulty: U256) -> Result<(), ApiError>;
    fn truncate_total_difficulty(&self, from: BlockNumber) -> Result<(), ApiError>;

    fn read_receipt(&self, number: Option<TxNumber>) -> Result<Receipt, ApiError>;
    fn write_receipts(&self, tx: Vec<TxNumber>, receipt: Vec<Receipt>) -> Result<(), ApiError>;
    fn delete_receipts(&self, tx: Vec<TxNumber>) -> Result<(), ApiError>;

    fn read_account(&self, address: Address, tx_number: Option<TxNumber>) -> Result<Account, ApiError>;
    fn read_account_code(&self, address: Address, tx_number: Option<TxNumber>) -> Result<Bytes, ApiError>;
    fn read_account_code_size(&self, address: Address, tx_number: Option<TxNumber>) -> Result<u64, ApiError>;
    fn read_account_storage(&self, address: Address, tx_number: Option<TxNumber>, key: H256) -> Result<Bytes, ApiError>;
    fn read_account_incarnation(&self, address: Address, tx_number: Option<TxNumber>) -> Result<u64, ApiError>;
    fn update_account_code(&self, address: Address, code: Bytes) -> Result<(), ApiError>;
    fn write_account_storage(&self, address: Address, key: H256, value: H256) -> Result<(), ApiError>;
    fn delete_account(&self, account: Address) -> Result<(), ApiError>;
    fn create_contract(&self, account: Address) -> Result<(), ApiError>;

    fn read_change_set_account(&self) -> Result<(), ApiError>;
    fn write_change_set_storage(&self) -> Result<(), ApiError>;

    fn read_history_account(&self) -> Result<(), ApiError>;
    fn write_history_storage(&self) -> Result<(), ApiError>;

}
