use crate::{
    db::{
        models::{BlockNumHash, StoredBlockOmmers},
        tables, DbTx, DbTxMut,
    },
    provider::Error as ProviderError,
    Result,
};
use auto_impl::auto_impl;
use reth_primitives::{
    rpc::{BlockId, BlockNumber},
    Block, BlockHash, BlockHashOrNumber, BlockLocked, Header, H256, U256,
};

/// Client trait for fetching `Header` related data.
#[auto_impl(&)]
pub trait HeaderProvider: Send + Sync {
    /// Check if block is known
    fn is_known(&self, block_hash: &BlockHash) -> Result<bool> {
        self.header(block_hash).map(|header| header.is_some())
    }

    /// Get header by block hash
    fn header(&self, block_hash: &BlockHash) -> Result<Option<Header>>;

    /// Get header by block number
    fn header_by_number(&self, num: u64) -> Result<Option<Header>>;

    /// Get header by block number or hash
    fn header_by_hash_or_number(&self, hash_or_num: BlockHashOrNumber) -> Result<Option<Header>> {
        match hash_or_num {
            BlockHashOrNumber::Hash(hash) => self.header(&hash),
            BlockHashOrNumber::Number(num) => self.header_by_number(num),
        }
    }
}

/// Api trait for fetching `Block` related data.
pub trait BlockProvider: Send + Sync {
    /// Returns the current info for the chain.
    fn chain_info(&self) -> Result<ChainInfo>;

    /// Returns the block. Returns `None` if block is not found.
    fn block(&self, id: BlockId) -> Result<Option<Block>>;

    /// Converts the `BlockNumber` variants.
    fn convert_block_number(
        &self,
        num: BlockNumber,
    ) -> Result<Option<reth_primitives::BlockNumber>> {
        let num = match num {
            BlockNumber::Latest => self.chain_info()?.best_number,
            BlockNumber::Earliest => 0,
            BlockNumber::Pending => return Ok(None),
            BlockNumber::Number(num) => num.as_u64(),
            BlockNumber::Finalized => return Ok(self.chain_info()?.last_finalized),
            BlockNumber::Safe => return Ok(self.chain_info()?.safe_finalized),
        };
        Ok(Some(num))
    }

    /// Get the hash of the block by matching the given id.
    fn block_hash_for_id(&self, block_id: BlockId) -> Result<Option<H256>> {
        match block_id {
            BlockId::Hash(hash) => Ok(Some(hash)),
            BlockId::Number(num) => {
                if matches!(num, BlockNumber::Latest) {
                    return Ok(Some(self.chain_info()?.best_hash))
                }
                self.convert_block_number(num)?
                    .map(|num| self.block_hash(num.into()))
                    .transpose()
                    .map(|maybe_hash| maybe_hash.flatten())
            }
        }
    }

    /// Get the number of the block by matching the given id.
    fn block_number_for_id(
        &self,
        block_id: BlockId,
    ) -> Result<Option<reth_primitives::BlockNumber>> {
        match block_id {
            BlockId::Hash(hash) => self.block_number(hash),
            BlockId::Number(num) => self.convert_block_number(num),
        }
    }

    /// Gets the `Block` for the given hash. Returns `None` if no block with this hash exists.
    fn block_number(&self, hash: H256) -> Result<Option<reth_primitives::BlockNumber>>;

    /// Get the hash of the block with the given number. Returns `None` if no block with this number
    /// exists.
    fn block_hash(&self, number: U256) -> Result<Option<H256>>;
}

/// Current status of the blockchain's head.
#[derive(Debug, Eq, PartialEq)]
pub struct ChainInfo {
    /// Best block hash.
    pub best_hash: H256,
    /// Best block number.
    pub best_number: reth_primitives::BlockNumber,
    /// Last block that was finalized.
    pub last_finalized: Option<reth_primitives::BlockNumber>,
    /// Safe block
    pub safe_finalized: Option<reth_primitives::BlockNumber>,
}

/// Get value from [tables::CumulativeTxCount] by block hash
/// as the table is indexed by NumHash key we are obtaining number from
/// [tables::HeaderNumbers]
pub fn get_cumulative_tx_count_by_hash<'a, TX: DbTxMut<'a> + DbTx<'a>>(
    tx: &TX,
    block_hash: H256,
) -> Result<u64> {
    let block_number = tx
        .get::<tables::HeaderNumbers>(block_hash)?
        .ok_or(ProviderError::BlockHashNotExist { block_hash })?;

    let block_num_hash = BlockNumHash((block_number, block_hash));

    tx.get::<tables::CumulativeTxCount>(block_num_hash)?
        .ok_or_else(|| ProviderError::BlockBodyNotExist { block_num_hash }.into())
}

/// Fill block to database. Useful for tests.
/// Check parent dependency in [tables::HeaderNumbers] and in [tables::CumulativeTxCount] tables.
/// Inserts blocks data to [tables::CanonicalHeaders], [tables::Headers], [tables::HeaderNumbers],
/// and transactions data to [tables::TxSenders], [tables::Transactions],
/// [tables::CumulativeTxCount] and [tables::BlockBodies]
pub fn insert_canonical_block<'a, TX: DbTxMut<'a> + DbTx<'a>>(
    tx: &TX,
    block: &BlockLocked,
) -> Result<()> {
    let block_num_hash = BlockNumHash((block.number, block.hash()));
    tx.put::<tables::CanonicalHeaders>(block.number, block.hash())?;
    // Put header with canonical hashes.
    tx.put::<tables::Headers>(block_num_hash, block.header.as_ref().clone())?;
    tx.put::<tables::HeaderNumbers>(block.hash(), block.number)?;

    let start_tx_number =
        if block.number == 0 { 0 } else { get_cumulative_tx_count_by_hash(tx, block.parent_hash)? };

    // insert body ommers data
    tx.put::<tables::BlockOmmers>(
        block_num_hash,
        StoredBlockOmmers { ommers: block.ommers.iter().map(|h| h.as_ref().clone()).collect() },
    )?;

    let mut tx_number = start_tx_number;
    for eth_tx in block.body.iter() {
        let rec_tx = eth_tx.clone().into_ecrecovered().unwrap();
        tx.put::<tables::TxSenders>(tx_number, rec_tx.signer())?;
        tx.put::<tables::Transactions>(tx_number, rec_tx.as_ref().clone())?;
        tx_number += 1;
    }

    tx.put::<tables::CumulativeTxCount>(block_num_hash, tx_number)?;

    Ok(())
}
