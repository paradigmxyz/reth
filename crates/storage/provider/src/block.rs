use auto_impl::auto_impl;
use reth_db::{
    models::{BlockNumHash, StoredBlockBody, StoredBlockOmmers},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::{provider::Error as ProviderError, Result};
use reth_primitives::{
    rpc::{BlockId, BlockNumber},
    Block, BlockHash, BlockHashOrNumber, Header, SealedBlock, H256, U256,
};

/// Client trait for fetching block hashes by number.
#[auto_impl(&)]
pub trait BlockHashProvider: Send + Sync {
    /// Get the hash of the block with the given number. Returns `None` if no block with this number
    /// exists.
    fn block_hash(&self, number: U256) -> Result<Option<H256>>;
}

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

    /// Get total difficulty by block hash.
    fn header_td(&self, hash: &BlockHash) -> Result<Option<U256>>;
}

/// Api trait for fetching `Block` related data.
pub trait BlockProvider: BlockHashProvider + Send + Sync {
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
            BlockId::Hash(hash) => Ok(Some(H256(hash.0))),
            BlockId::Number(num) => {
                if matches!(num, BlockNumber::Latest) {
                    return Ok(Some(self.chain_info()?.best_hash))
                }
                self.convert_block_number(num)?
                    .map(|num| self.block_hash(U256::from(num)))
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
            BlockId::Hash(hash) => self.block_number(H256(hash.0)),
            BlockId::Number(num) => self.convert_block_number(num),
        }
    }

    /// Gets the `Block` for the given hash. Returns `None` if no block with this hash exists.
    fn block_number(&self, hash: H256) -> Result<Option<reth_primitives::BlockNumber>>;
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

/// Fill block to database. Useful for tests.
/// Check parent dependency in [tables::HeaderNumbers] and in [tables::CumulativeTxCount] tables.
/// Inserts blocks data to [tables::CanonicalHeaders], [tables::Headers], [tables::HeaderNumbers],
/// and transactions data to [tables::TxSenders], [tables::Transactions],
/// [tables::CumulativeTxCount] and [tables::BlockBodies]
pub fn insert_block<'a, TX: DbTxMut<'a> + DbTx<'a>>(
    tx: &TX,
    block: &SealedBlock,
    has_block_reward: bool,
    parent_tx_num_transition_id: Option<(u64, u64)>,
) -> Result<()> {
    let block_num_hash = BlockNumHash((block.number, block.hash()));
    tx.put::<tables::CanonicalHeaders>(block.number, block.hash())?;
    // Put header with canonical hashes.
    tx.put::<tables::Headers>(block_num_hash, block.header.as_ref().clone())?;
    tx.put::<tables::HeaderNumbers>(block.hash(), block.number)?;

    // insert body ommers data
    tx.put::<tables::BlockOmmers>(
        block_num_hash,
        StoredBlockOmmers { ommers: block.ommers.iter().map(|h| h.as_ref().clone()).collect() },
    )?;

    let (mut current_tx_id, mut transition_id) =
        if let Some(parent_tx_num_transition_id) = parent_tx_num_transition_id {
            parent_tx_num_transition_id
        } else if block.number == 0 {
            (0, 0)
        } else {
            let prev_block_num = block.number - 1;
            let prev_block_hash = tx
                .get::<tables::CanonicalHeaders>(prev_block_num)?
                .ok_or(ProviderError::BlockNumber { block_number: prev_block_num })?;
            let prev_body = tx
                .get::<tables::BlockBodies>((prev_block_num, prev_block_hash).into())?
                .ok_or(ProviderError::BlockBody {
                    block_number: prev_block_num,
                    block_hash: prev_block_hash,
                })?;
            let last_transition_id = tx
                .get::<tables::BlockTransitionIndex>(prev_block_num)?
                .ok_or(ProviderError::BlockTransition { block_number: prev_block_num })?;
            (prev_body.start_tx_id + prev_body.tx_count, last_transition_id)
        };

    // insert body data
    tx.put::<tables::BlockBodies>(
        block_num_hash,
        StoredBlockBody { start_tx_id: current_tx_id, tx_count: block.body.len() as u64 },
    )?;

    for transaction in block.body.iter() {
        let rec_tx = transaction.clone().into_ecrecovered().unwrap();
        tx.put::<tables::TxSenders>(current_tx_id, rec_tx.signer())?;
        tx.put::<tables::Transactions>(current_tx_id, rec_tx.into())?;
        tx.put::<tables::TxTransitionIndex>(current_tx_id, transition_id)?;
        transition_id += 1;
        current_tx_id += 1;
    }

    if has_block_reward {
        transition_id += 1;
    }
    tx.put::<tables::BlockTransitionIndex>(block.number, transition_id)?;

    Ok(())
}

/// Inserts canonical block in blockchain. Parent tx num and transition id is taken from
/// parent block in database.
pub fn insert_canonical_block<'a, TX: DbTxMut<'a> + DbTx<'a>>(
    tx: &TX,
    block: &SealedBlock,
    has_block_reward: bool,
) -> Result<()> {
    insert_block(tx, block, has_block_reward, None)
}
