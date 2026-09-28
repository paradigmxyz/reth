use super::StageId;
#[cfg(test)]
use alloc::vec;
use alloc::{format, string::String, vec::Vec};
use alloy_primitives::{Address, BlockNumber, B256, U256};
use core::ops::RangeInclusive;
use reth_primitives_traits::Account;
use reth_trie_common::{hash_builder::HashBuilderState, StoredSubNode};

/// Saves the progress of Merkle stage.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct MerkleCheckpoint {
    /// The target block number.
    pub target_block: BlockNumber,
    /// The last hashed account key processed.
    pub last_account_key: B256,
    /// Previously recorded walker stack.
    pub walker_stack: Vec<StoredSubNode>,
    /// The hash builder state.
    pub state: HashBuilderState,
    /// Optional storage root checkpoint for the last processed account.
    pub storage_root_checkpoint: Option<StorageRootMerkleCheckpoint>,
}

impl MerkleCheckpoint {
    /// Creates a new Merkle checkpoint.
    pub const fn new(
        target_block: BlockNumber,
        last_account_key: B256,
        walker_stack: Vec<StoredSubNode>,
        state: HashBuilderState,
    ) -> Self {
        Self { target_block, last_account_key, walker_stack, state, storage_root_checkpoint: None }
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for MerkleCheckpoint {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let mut len = 0;

        buf.put_u64(self.target_block);
        len += 8;

        buf.put_slice(self.last_account_key.as_slice());
        len += self.last_account_key.len();

        buf.put_u16(self.walker_stack.len() as u16);
        len += 2;
        for item in &self.walker_stack {
            len += item.to_compact(buf);
        }

        len += self.state.to_compact(buf);

        // Encode the optional storage root checkpoint
        match &self.storage_root_checkpoint {
            Some(checkpoint) => {
                // one means Some
                buf.put_u8(1);
                len += 1;
                len += checkpoint.to_compact(buf);
            }
            None => {
                // zero means None
                buf.put_u8(0);
                len += 1;
            }
        }

        len
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        use bytes::Buf;
        let (mut buf, trailing) = if len == 0 { (buf, &[][..]) } else { buf.split_at(len) };
        let target_block = buf.get_u64();

        let last_account_key = B256::from_slice(&buf[..32]);
        buf.advance(32);

        let walker_stack_len = buf.get_u16() as usize;
        let mut walker_stack = Vec::with_capacity(walker_stack_len);
        for _ in 0..walker_stack_len {
            let (item, rest) = StoredSubNode::from_compact(buf, 0);
            walker_stack.push(item);
            buf = rest;
        }

        let (state, mut buf) = HashBuilderState::from_compact(buf, 0);

        // Decode the storage root checkpoint if it exists
        let (storage_root_checkpoint, buf) = if buf.is_empty() {
            (None, buf)
        } else {
            match buf.get_u8() {
                1 => {
                    let (checkpoint, rest) = StorageRootMerkleCheckpoint::from_compact(buf, 0);
                    (Some(checkpoint), rest)
                }
                _ => (None, buf),
            }
        };

        (
            Self { target_block, last_account_key, walker_stack, state, storage_root_checkpoint },
            if len == 0 { buf } else { trailing },
        )
    }
}

/// Saves the progress of a storage root computation.
///
/// This contains the walker stack, hash builder state, and the last storage key processed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageRootMerkleCheckpoint {
    /// The last storage key processed.
    pub last_storage_key: B256,
    /// Previously recorded walker stack.
    pub walker_stack: Vec<StoredSubNode>,
    /// The hash builder state.
    pub state: HashBuilderState,
    /// The account nonce.
    pub account_nonce: u64,
    /// The account balance.
    pub account_balance: U256,
    /// The account bytecode hash.
    pub account_bytecode_hash: B256,
    /// Payload of the account whose storage root is in progress.
    #[cfg(feature = "account-ext")]
    pub account_extension: reth_primitives_traits::AccountExtension,
}

impl StorageRootMerkleCheckpoint {
    /// Creates a new storage root merkle checkpoint.
    pub fn new(
        last_storage_key: B256,
        walker_stack: Vec<StoredSubNode>,
        state: HashBuilderState,
        account: Account,
    ) -> Self {
        Self {
            last_storage_key,
            walker_stack,
            state,
            account_nonce: account.nonce,
            account_balance: account.balance,
            account_bytecode_hash: account.get_bytecode_hash(),
            #[cfg(feature = "account-ext")]
            account_extension: account.extension,
        }
    }

    /// Returns the account whose storage root is in progress.
    #[allow(clippy::needless_update)]
    pub fn account(&self) -> Account {
        Account {
            nonce: self.account_nonce,
            balance: self.account_balance,
            bytecode_hash: Some(self.account_bytecode_hash),
            #[cfg(feature = "account-ext")]
            extension: self.account_extension.clone(),
            ..Default::default()
        }
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for StorageRootMerkleCheckpoint {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let mut len = 0;

        buf.put_slice(self.last_storage_key.as_slice());
        len += self.last_storage_key.len();

        buf.put_u16(self.walker_stack.len() as u16);
        len += 2;
        for item in &self.walker_stack {
            len += item.to_compact(buf);
        }

        len += self.state.to_compact(buf);

        // Encode account fields
        buf.put_u64(self.account_nonce);
        len += 8;

        let balance_len = self.account_balance.byte_len() as u8;
        buf.put_u8(balance_len);
        len += 1;
        len += self.account_balance.to_compact(buf);

        buf.put_slice(self.account_bytecode_hash.as_slice());
        len += 32;
        #[cfg(feature = "account-ext")]
        if !self.account_extension.is_empty() {
            let extension_len = u16::try_from(self.account_extension.len())
                .expect("account extension exceeds compact encoding limit");
            buf.put_u16(extension_len);
            len += 2;
            buf.put_slice(&self.account_extension);
            len += self.account_extension.len();
        }

        len
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        use bytes::Buf;
        let (mut buf, trailing) = if len == 0 { (buf, &[][..]) } else { buf.split_at(len) };

        let last_storage_key = B256::from_slice(&buf[..32]);
        buf.advance(32);

        let walker_stack_len = buf.get_u16() as usize;
        let mut walker_stack = Vec::with_capacity(walker_stack_len);
        for _ in 0..walker_stack_len {
            let (item, rest) = StoredSubNode::from_compact(buf, 0);
            walker_stack.push(item);
            buf = rest;
        }

        let (state, mut buf) = HashBuilderState::from_compact(buf, 0);

        // Decode account fields
        let account_nonce = buf.get_u64();
        let balance_len = buf.get_u8() as usize;
        let (account_balance, mut buf) = U256::from_compact(buf, balance_len);
        let account_bytecode_hash = B256::from_slice(&buf[..32]);
        buf.advance(32);
        #[cfg(feature = "account-ext")]
        let (account_extension, buf) = if buf.is_empty() {
            (reth_primitives_traits::AccountExtension::default(), buf)
        } else {
            let account_extension_len = buf.get_u16() as usize;
            let account_extension = reth_primitives_traits::AccountExtension::copy_from_slice(
                &buf[..account_extension_len],
            );
            (account_extension, &buf[account_extension_len..])
        };

        (
            Self {
                last_storage_key,
                walker_stack,
                state,
                account_nonce,
                account_balance,
                account_bytecode_hash,
                #[cfg(feature = "account-ext")]
                account_extension,
            },
            if len == 0 { buf } else { trailing },
        )
    }
}

/// Saves the progress of `AccountHashing` stage.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AccountHashingCheckpoint {
    /// The next account to start hashing from.
    pub address: Option<Address>,
    /// Block range which this checkpoint is valid for.
    pub block_range: CheckpointBlockRange,
    /// Progress measured in accounts.
    pub progress: EntitiesCheckpoint,
}

/// Saves the progress of `StorageHashing` stage.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StorageHashingCheckpoint {
    /// The next account to start hashing from.
    pub address: Option<Address>,
    /// The next storage slot to start hashing from.
    pub storage: Option<B256>,
    /// Block range which this checkpoint is valid for.
    pub block_range: CheckpointBlockRange,
    /// Progress measured in storage slots.
    pub progress: EntitiesCheckpoint,
}

/// Saves the progress of Execution stage.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ExecutionCheckpoint {
    /// Block range which this checkpoint is valid for.
    pub block_range: CheckpointBlockRange,
    /// Progress measured in gas.
    pub progress: EntitiesCheckpoint,
}

/// Saves the progress of Headers stage.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HeadersCheckpoint {
    /// Block range which this checkpoint is valid for.
    pub block_range: CheckpointBlockRange,
    /// Progress measured in gas.
    pub progress: EntitiesCheckpoint,
}

/// Saves the progress of Index History stages.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IndexHistoryCheckpoint {
    /// Block range which this checkpoint is valid for.
    pub block_range: CheckpointBlockRange,
    /// Progress measured in changesets.
    pub progress: EntitiesCheckpoint,
}

/// Saves the progress of `MerkleChangeSets` stage.
///
/// Note: This type is only kept for backward compatibility with the Compact codec.
/// The `MerkleChangeSets` stage has been removed.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MerkleChangeSetsCheckpoint {
    /// Block range which this checkpoint is valid for.
    pub block_range: CheckpointBlockRange,
}

/// Saves the progress of abstract stage iterating over or downloading entities.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EntitiesCheckpoint {
    /// Number of entities already processed.
    pub processed: u64,
    /// Total entities to be processed.
    pub total: u64,
}

impl EntitiesCheckpoint {
    /// Formats entities checkpoint as percentage, i.e. `processed / total`.
    ///
    /// Return [None] if `total == 0`.
    pub fn fmt_percentage(&self) -> Option<String> {
        if self.total == 0 {
            return None
        }

        // Calculate percentage with 2 decimal places.
        let percentage = 100.0 * self.processed as f64 / self.total as f64;

        // Truncate to 2 decimal places, rounding down so that 99.999% becomes 99.99% and not 100%.
        #[cfg(not(feature = "std"))]
        {
            // Manual floor implementation using integer arithmetic for no_std
            let scaled = (percentage * 100.0) as u64;
            Some(format!("{:.2}%", scaled as f64 / 100.0))
        }
        #[cfg(feature = "std")]
        Some(format!("{:.2}%", (percentage * 100.0).floor() / 100.0))
    }
}

/// Saves the block range. Usually, it's used to check the validity of some stage checkpoint across
/// multiple executions.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CheckpointBlockRange {
    /// The first block of the range, inclusive.
    pub from: BlockNumber,
    /// The last block of the range, inclusive.
    pub to: BlockNumber,
}

impl From<RangeInclusive<BlockNumber>> for CheckpointBlockRange {
    fn from(range: RangeInclusive<BlockNumber>) -> Self {
        Self { from: *range.start(), to: *range.end() }
    }
}

impl From<&RangeInclusive<BlockNumber>> for CheckpointBlockRange {
    fn from(range: &RangeInclusive<BlockNumber>) -> Self {
        Self { from: *range.start(), to: *range.end() }
    }
}

/// Saves the progress of a stage.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StageCheckpoint {
    /// The maximum block processed by the stage.
    pub block_number: BlockNumber,
    /// Stage-specific checkpoint. None if stage uses only block-based checkpoints.
    pub stage_checkpoint: Option<StageUnitCheckpoint>,
}

impl StageCheckpoint {
    /// Creates a new [`StageCheckpoint`] with only `block_number` set.
    pub fn new(block_number: BlockNumber) -> Self {
        Self { block_number, ..Default::default() }
    }

    /// Sets the block number.
    pub const fn with_block_number(mut self, block_number: BlockNumber) -> Self {
        self.block_number = block_number;
        self
    }

    /// Sets the block range, if checkpoint uses block range.
    pub fn with_block_range(mut self, stage_id: &StageId, from: u64, to: u64) -> Self {
        self.stage_checkpoint = Some(match stage_id {
            StageId::Execution => StageUnitCheckpoint::Execution(ExecutionCheckpoint::default()),
            StageId::AccountHashing => {
                StageUnitCheckpoint::Account(AccountHashingCheckpoint::default())
            }
            StageId::StorageHashing => {
                StageUnitCheckpoint::Storage(StorageHashingCheckpoint::default())
            }
            StageId::IndexStorageHistory | StageId::IndexAccountHistory => {
                StageUnitCheckpoint::IndexHistory(IndexHistoryCheckpoint::default())
            }
            _ => return self,
        });
        if let Some(ref mut checkpoint) = self.stage_checkpoint {
            checkpoint.set_block_range(from, to);
        }
        self
    }

    /// Get the underlying [`EntitiesCheckpoint`], if any, to determine the number of entities
    /// processed, and the number of total entities to process.
    pub fn entities(&self) -> Option<EntitiesCheckpoint> {
        let stage_checkpoint = self.stage_checkpoint?;

        match stage_checkpoint {
            StageUnitCheckpoint::Account(AccountHashingCheckpoint {
                progress: entities, ..
            }) |
            StageUnitCheckpoint::Storage(StorageHashingCheckpoint {
                progress: entities, ..
            }) |
            StageUnitCheckpoint::Entities(entities) |
            StageUnitCheckpoint::Execution(ExecutionCheckpoint { progress: entities, .. }) |
            StageUnitCheckpoint::Headers(HeadersCheckpoint { progress: entities, .. }) |
            StageUnitCheckpoint::IndexHistory(IndexHistoryCheckpoint {
                progress: entities,
                ..
            }) => Some(entities),
            StageUnitCheckpoint::MerkleChangeSets(_) | StageUnitCheckpoint::Finish(_) => None,
        }
    }
}

#[cfg(any(test, feature = "reth-codec"))]
reth_codecs::impl_compression_for_compact!(StageCheckpoint);

/// Saves the progress of the Finish stage.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FinishCheckpoint {
    /// The highest block with a partially persisted state and trie.
    pub partial_state_trie: Option<BlockNumber>,
}

impl FinishCheckpoint {
    /// Returns the highest block with a partially persisted state and trie.
    pub const fn partial_state_trie(&self) -> Option<BlockNumber> {
        self.partial_state_trie
    }
}

// TODO(alexey): add a merkle checkpoint. Currently it's hard because [`MerkleCheckpoint`]
//  is not a Copy type.
/// Stage-specific checkpoint metrics.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum StageUnitCheckpoint {
    /// Saves the progress of `AccountHashing` stage.
    Account(AccountHashingCheckpoint),
    /// Saves the progress of `StorageHashing` stage.
    Storage(StorageHashingCheckpoint),
    /// Saves the progress of abstract stage iterating over or downloading entities.
    Entities(EntitiesCheckpoint),
    /// Saves the progress of Execution stage.
    Execution(ExecutionCheckpoint),
    /// Saves the progress of Headers stage.
    Headers(HeadersCheckpoint),
    /// Saves the progress of Index History stage.
    IndexHistory(IndexHistoryCheckpoint),
    /// Saves the progress of `MerkleChangeSets` stage.
    ///
    /// Note: This variant is only kept for backward compatibility with the Compact codec.
    /// The `MerkleChangeSets` stage has been removed.
    MerkleChangeSets(MerkleChangeSetsCheckpoint),
    /// Saves the progress of the Finish stage.
    Finish(FinishCheckpoint),
}

impl StageUnitCheckpoint {
    /// Sets the block range. Returns old block range, or `None` if checkpoint doesn't use block
    /// range.
    pub const fn set_block_range(&mut self, from: u64, to: u64) -> Option<CheckpointBlockRange> {
        match self {
            Self::Account(AccountHashingCheckpoint { block_range, .. }) |
            Self::Storage(StorageHashingCheckpoint { block_range, .. }) |
            Self::Execution(ExecutionCheckpoint { block_range, .. }) |
            Self::IndexHistory(IndexHistoryCheckpoint { block_range, .. }) => {
                let old_range = *block_range;
                *block_range = CheckpointBlockRange { from, to };

                Some(old_range)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
impl Default for StageUnitCheckpoint {
    fn default() -> Self {
        Self::Account(AccountHashingCheckpoint::default())
    }
}

/// Generates [`StageCheckpoint`] getter and builder methods.
macro_rules! stage_unit_checkpoints {
    ($(($index:expr,$enum_variant:tt,$checkpoint_ty:ty,#[doc = $fn_get_doc:expr]$fn_get_name:ident,#[doc = $fn_build_doc:expr]$fn_build_name:ident)),+) => {
        impl StageCheckpoint {
            $(
                #[doc = $fn_get_doc]
                pub const fn $fn_get_name(&self) -> Option<$checkpoint_ty> {
                    match self.stage_checkpoint {
                        Some(StageUnitCheckpoint::$enum_variant(checkpoint)) => Some(checkpoint),
                        _ => None,
                    }
                }

                #[doc = $fn_build_doc]
                pub const fn $fn_build_name(
                    mut self,
                    checkpoint: $checkpoint_ty,
                ) -> Self {
                    self.stage_checkpoint = Some(StageUnitCheckpoint::$enum_variant(checkpoint));
                    self
                }
            )+
        }
    };
}

stage_unit_checkpoints!(
    (
        0,
        Account,
        AccountHashingCheckpoint,
        /// Returns the account hashing stage checkpoint, if any.
        account_hashing_stage_checkpoint,
        /// Sets the stage checkpoint to account hashing.
        with_account_hashing_stage_checkpoint
    ),
    (
        1,
        Storage,
        StorageHashingCheckpoint,
        /// Returns the storage hashing stage checkpoint, if any.
        storage_hashing_stage_checkpoint,
        /// Sets the stage checkpoint to storage hashing.
        with_storage_hashing_stage_checkpoint
    ),
    (
        2,
        Entities,
        EntitiesCheckpoint,
        /// Returns the entities stage checkpoint, if any.
        entities_stage_checkpoint,
        /// Sets the stage checkpoint to entities.
        with_entities_stage_checkpoint
    ),
    (
        3,
        Execution,
        ExecutionCheckpoint,
        /// Returns the execution stage checkpoint, if any.
        execution_stage_checkpoint,
        /// Sets the stage checkpoint to execution.
        with_execution_stage_checkpoint
    ),
    (
        4,
        Headers,
        HeadersCheckpoint,
        /// Returns the headers stage checkpoint, if any.
        headers_stage_checkpoint,
        /// Sets the stage checkpoint to headers.
        with_headers_stage_checkpoint
    ),
    (
        5,
        IndexHistory,
        IndexHistoryCheckpoint,
        /// Returns the index history stage checkpoint, if any.
        index_history_stage_checkpoint,
        /// Sets the stage checkpoint to index history.
        with_index_history_stage_checkpoint
    ),
    (
        6,
        Finish,
        FinishCheckpoint,
        /// Returns the finish stage checkpoint, if any.
        finish_stage_checkpoint,
        /// Sets the stage checkpoint to finish.
        with_finish_stage_checkpoint
    )
);

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::b256;
    use proptest::{collection::vec, option, prelude::*};
    use rand::Rng;
    use reth_codecs::Compact;
    use reth_trie_common::{RlpNode, TrieMask};

    #[test]
    fn merkle_checkpoint_roundtrip() {
        let mut rng = rand::rng();
        let checkpoint = MerkleCheckpoint {
            target_block: rng.random(),
            last_account_key: rng.random(),
            walker_stack: vec![StoredSubNode {
                key: B256::random_with(&mut rng).to_vec(),
                nibble: Some(rng.random()),
                node: None,
            }],
            state: HashBuilderState::default(),
            storage_root_checkpoint: None,
        };

        let mut buf = Vec::new();
        let encoded = checkpoint.to_compact(&mut buf);
        let (decoded, _) = MerkleCheckpoint::from_compact(&buf, encoded);
        assert_eq!(decoded, checkpoint);
    }

    #[test]
    fn storage_root_merkle_checkpoint_roundtrip() {
        let mut rng = rand::rng();
        let checkpoint = StorageRootMerkleCheckpoint {
            last_storage_key: rng.random(),
            walker_stack: vec![StoredSubNode {
                key: B256::random_with(&mut rng).to_vec(),
                nibble: Some(rng.random()),
                node: None,
            }],
            state: HashBuilderState::default(),
            account_nonce: 0,
            account_balance: U256::ZERO,
            account_bytecode_hash: B256::ZERO,
            #[cfg(feature = "account-ext")]
            account_extension: vec![0x42; 32].into(),
        };

        let mut buf = Vec::new();
        let encoded = checkpoint.to_compact(&mut buf);
        assert_eq!(encoded, buf.len());

        #[cfg(feature = "account-ext")]
        {
            let mut empty_checkpoint = checkpoint.clone();
            empty_checkpoint.account_extension = Default::default();
            let mut empty_buf = Vec::new();
            let empty_encoded = empty_checkpoint.to_compact(&mut empty_buf);
            assert_eq!(encoded, empty_encoded + 2 + checkpoint.account_extension.len());
            assert_eq!(empty_encoded, empty_buf.len());
            empty_buf.extend_from_slice(&[0x12, 0x34]);
            let (decoded, rest) =
                StorageRootMerkleCheckpoint::from_compact(&empty_buf, empty_encoded);
            assert_eq!(decoded, empty_checkpoint);
            assert_eq!(rest, &[0x12, 0x34]);
        }

        buf.extend_from_slice(&[0x12, 0x34]);
        let (decoded, rest) = StorageRootMerkleCheckpoint::from_compact(&buf, encoded);
        assert_eq!(decoded, checkpoint);
        assert_eq!(rest, &[0x12, 0x34]);
    }

    #[test]
    fn merkle_checkpoint_with_storage_root_roundtrip() {
        let mut rng = rand::rng();

        // Create a storage root checkpoint
        let storage_checkpoint = StorageRootMerkleCheckpoint {
            last_storage_key: rng.random(),
            walker_stack: vec![StoredSubNode {
                key: B256::random_with(&mut rng).to_vec(),
                nibble: Some(rng.random()),
                node: None,
            }],
            state: HashBuilderState::default(),
            account_nonce: 1,
            account_balance: U256::from(1),
            account_bytecode_hash: b256!(
                "0x0fffffffffffffffffffffffffffffff0fffffffffffffffffffffffffffffff"
            ),
            #[cfg(feature = "account-ext")]
            account_extension: vec![0x42; 32].into(),
        };

        // Create a merkle checkpoint with the storage root checkpoint
        let checkpoint = MerkleCheckpoint {
            target_block: rng.random(),
            last_account_key: rng.random(),
            walker_stack: vec![StoredSubNode {
                key: B256::random_with(&mut rng).to_vec(),
                nibble: Some(rng.random()),
                node: None,
            }],
            state: HashBuilderState::default(),
            storage_root_checkpoint: Some(storage_checkpoint),
        };

        let mut buf = Vec::new();
        let encoded = checkpoint.to_compact(&mut buf);
        let (decoded, _) = MerkleCheckpoint::from_compact(&buf, encoded);
        assert_eq!(decoded, checkpoint);
    }

    #[test]
    fn finish_checkpoint_roundtrip() {
        let finish_checkpoint = FinishCheckpoint { partial_state_trie: Some(21) };
        let checkpoint = StageCheckpoint::new(42).with_finish_stage_checkpoint(finish_checkpoint);

        let mut buf = Vec::new();
        let encoded = checkpoint.to_compact(&mut buf);
        let (decoded, _) = StageCheckpoint::from_compact(&buf, encoded);

        assert_eq!(decoded, checkpoint);
        assert_eq!(decoded.finish_stage_checkpoint().unwrap().partial_state_trie(), Some(21));
    }

    /// Bytes following a checkpoint record, to check that decoding stays within the record.
    const TRAILING: [u8; 2] = [0xab, 0xcd];

    /// [`StorageRootMerkleCheckpoint`] with its encoding from before account extensions.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct LegacyStorageRootMerkleCheckpoint {
        last_storage_key: B256,
        walker_stack: Vec<StoredSubNode>,
        state: HashBuilderState,
        account_nonce: u64,
        account_balance: U256,
        account_bytecode_hash: B256,
    }

    impl Compact for LegacyStorageRootMerkleCheckpoint {
        fn to_compact<B>(&self, buf: &mut B) -> usize
        where
            B: bytes::BufMut + AsMut<[u8]>,
        {
            let mut len = 0;

            buf.put_slice(self.last_storage_key.as_slice());
            len += self.last_storage_key.len();

            buf.put_u16(self.walker_stack.len() as u16);
            len += 2;
            for item in &self.walker_stack {
                len += item.to_compact(buf);
            }

            len += self.state.to_compact(buf);

            buf.put_u64(self.account_nonce);
            len += 8;

            let balance_len = self.account_balance.byte_len() as u8;
            buf.put_u8(balance_len);
            len += 1;
            len += self.account_balance.to_compact(buf);

            buf.put_slice(self.account_bytecode_hash.as_slice());
            len += 32;

            len
        }

        fn from_compact(mut buf: &[u8], _len: usize) -> (Self, &[u8]) {
            use bytes::Buf;

            let last_storage_key = B256::from_slice(&buf[..32]);
            buf.advance(32);

            let walker_stack_len = buf.get_u16() as usize;
            let mut walker_stack = Vec::with_capacity(walker_stack_len);
            for _ in 0..walker_stack_len {
                let (item, rest) = StoredSubNode::from_compact(buf, 0);
                walker_stack.push(item);
                buf = rest;
            }

            let (state, mut buf) = HashBuilderState::from_compact(buf, 0);

            let account_nonce = buf.get_u64();
            let balance_len = buf.get_u8() as usize;
            let (account_balance, mut buf) = U256::from_compact(buf, balance_len);
            let account_bytecode_hash = B256::from_slice(&buf[..32]);
            buf.advance(32);

            (
                Self {
                    last_storage_key,
                    walker_stack,
                    state,
                    account_nonce,
                    account_balance,
                    account_bytecode_hash,
                },
                buf,
            )
        }
    }

    impl From<LegacyStorageRootMerkleCheckpoint> for StorageRootMerkleCheckpoint {
        fn from(legacy: LegacyStorageRootMerkleCheckpoint) -> Self {
            Self {
                last_storage_key: legacy.last_storage_key,
                walker_stack: legacy.walker_stack,
                state: legacy.state,
                account_nonce: legacy.account_nonce,
                account_balance: legacy.account_balance,
                account_bytecode_hash: legacy.account_bytecode_hash,
                #[cfg(feature = "account-ext")]
                account_extension: Default::default(),
            }
        }
    }

    /// [`MerkleCheckpoint`] with its encoding from before account extensions.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct LegacyMerkleCheckpoint {
        target_block: BlockNumber,
        last_account_key: B256,
        walker_stack: Vec<StoredSubNode>,
        state: HashBuilderState,
        storage_root_checkpoint: Option<LegacyStorageRootMerkleCheckpoint>,
    }

    impl Compact for LegacyMerkleCheckpoint {
        fn to_compact<B>(&self, buf: &mut B) -> usize
        where
            B: bytes::BufMut + AsMut<[u8]>,
        {
            let mut len = 0;

            buf.put_u64(self.target_block);
            len += 8;

            buf.put_slice(self.last_account_key.as_slice());
            len += self.last_account_key.len();

            buf.put_u16(self.walker_stack.len() as u16);
            len += 2;
            for item in &self.walker_stack {
                len += item.to_compact(buf);
            }

            len += self.state.to_compact(buf);

            match &self.storage_root_checkpoint {
                Some(checkpoint) => {
                    buf.put_u8(1);
                    len += 1;
                    len += checkpoint.to_compact(buf);
                }
                None => {
                    buf.put_u8(0);
                    len += 1;
                }
            }

            len
        }

        fn from_compact(mut buf: &[u8], _len: usize) -> (Self, &[u8]) {
            use bytes::Buf;
            let target_block = buf.get_u64();

            let last_account_key = B256::from_slice(&buf[..32]);
            buf.advance(32);

            let walker_stack_len = buf.get_u16() as usize;
            let mut walker_stack = Vec::with_capacity(walker_stack_len);
            for _ in 0..walker_stack_len {
                let (item, rest) = StoredSubNode::from_compact(buf, 0);
                walker_stack.push(item);
                buf = rest;
            }

            let (state, mut buf) = HashBuilderState::from_compact(buf, 0);

            let (storage_root_checkpoint, buf) = if buf.is_empty() {
                (None, buf)
            } else {
                match buf.get_u8() {
                    1 => {
                        let (checkpoint, rest) =
                            LegacyStorageRootMerkleCheckpoint::from_compact(buf, 0);
                        (Some(checkpoint), rest)
                    }
                    _ => (None, buf),
                }
            };

            (
                Self {
                    target_block,
                    last_account_key,
                    walker_stack,
                    state,
                    storage_root_checkpoint,
                },
                buf,
            )
        }
    }

    impl From<LegacyMerkleCheckpoint> for MerkleCheckpoint {
        fn from(legacy: LegacyMerkleCheckpoint) -> Self {
            Self {
                target_block: legacy.target_block,
                last_account_key: legacy.last_account_key,
                walker_stack: legacy.walker_stack,
                state: legacy.state,
                storage_root_checkpoint: legacy.storage_root_checkpoint.map(Into::into),
            }
        }
    }

    // Walker entries carry no branch node: `StoredSubNode` decodes a branch node by consuming
    // the rest of the buffer, so one can only round-trip at the end of a record.
    fn stored_sub_node() -> impl Strategy<Value = StoredSubNode> {
        (vec(0u8..16, 0..=64), option::of(0u8..16)).prop_map(|(key, nibble)| StoredSubNode {
            key,
            nibble,
            node: None,
        })
    }

    fn hash_builder_state() -> impl Strategy<Value = HashBuilderState> {
        (
            vec(0u8..16, 0..=64),
            vec(any::<u8>(), 0..=64),
            vec(vec(any::<u8>(), 0..=32), 0..=8),
            vec(any::<u16>(), 0..=8),
            vec(any::<u16>(), 0..=8),
            vec(any::<u16>(), 0..=8),
            any::<bool>(),
        )
            .prop_map(
                |(key, value, stack, groups, tree_masks, hash_masks, stored_in_database)| {
                    let mut state = HashBuilderState {
                        key,
                        stack: stack.iter().map(|node| RlpNode::from_raw(node).unwrap()).collect(),
                        groups: groups.into_iter().map(TrieMask::new).collect(),
                        tree_masks: tree_masks.into_iter().map(TrieMask::new).collect(),
                        hash_masks: hash_masks.into_iter().map(TrieMask::new).collect(),
                        stored_in_database,
                        ..Default::default()
                    };
                    state.value.set_bytes_owned(value);
                    state
                },
            )
    }

    fn legacy_storage_root_checkpoint() -> impl Strategy<Value = LegacyStorageRootMerkleCheckpoint>
    {
        (
            any::<B256>(),
            vec(stored_sub_node(), 0..=4),
            hash_builder_state(),
            any::<u64>(),
            any::<U256>(),
            any::<B256>(),
        )
            .prop_map(
                |(
                    last_storage_key,
                    walker_stack,
                    state,
                    account_nonce,
                    account_balance,
                    account_bytecode_hash,
                )| {
                    LegacyStorageRootMerkleCheckpoint {
                        last_storage_key,
                        walker_stack,
                        state,
                        account_nonce,
                        account_balance,
                        account_bytecode_hash,
                    }
                },
            )
    }

    fn legacy_merkle_checkpoint() -> impl Strategy<Value = LegacyMerkleCheckpoint> {
        (
            any::<BlockNumber>(),
            any::<B256>(),
            vec(stored_sub_node(), 0..=4),
            hash_builder_state(),
            option::of(legacy_storage_root_checkpoint()),
        )
            .prop_map(
                |(target_block, last_account_key, walker_stack, state, storage_root_checkpoint)| {
                    LegacyMerkleCheckpoint {
                        target_block,
                        last_account_key,
                        walker_stack,
                        state,
                        storage_root_checkpoint,
                    }
                },
            )
    }

    proptest! {
        #[test]
        fn storage_root_merkle_checkpoint_matches_legacy_encoding(
            legacy in legacy_storage_root_checkpoint(),
        ) {
            let mut legacy_buf = Vec::new();
            let legacy_len = legacy.to_compact(&mut legacy_buf);
            prop_assert_eq!(legacy_len, legacy_buf.len());

            // Without an extension, the encoding is the legacy one.
            let checkpoint = StorageRootMerkleCheckpoint::from(legacy.clone());
            let mut buf = Vec::new();
            let len = checkpoint.to_compact(&mut buf);
            prop_assert_eq!(len, buf.len());
            prop_assert_eq!(&buf, &legacy_buf);

            // Legacy records decode unchanged and leave the following bytes alone.
            legacy_buf.extend_from_slice(&TRAILING);
            let (decoded, rest) = StorageRootMerkleCheckpoint::from_compact(&legacy_buf, legacy_len);
            prop_assert_eq!(decoded, checkpoint);
            prop_assert_eq!(rest, &TRAILING[..]);

            // The legacy decoder still reads records without an extension.
            let (decoded, _) = LegacyStorageRootMerkleCheckpoint::from_compact(&buf, len);
            prop_assert_eq!(decoded, legacy);
        }

        #[test]
        fn merkle_checkpoint_matches_legacy_encoding(legacy in legacy_merkle_checkpoint()) {
            let mut legacy_buf = Vec::new();
            let legacy_len = legacy.to_compact(&mut legacy_buf);
            prop_assert_eq!(legacy_len, legacy_buf.len());

            let checkpoint = MerkleCheckpoint::from(legacy.clone());
            let mut buf = Vec::new();
            let len = checkpoint.to_compact(&mut buf);
            prop_assert_eq!(len, buf.len());
            prop_assert_eq!(&buf, &legacy_buf);

            // The merkle stage decodes the whole stored value.
            let (decoded, rest) = MerkleCheckpoint::from_compact(&legacy_buf, legacy_buf.len());
            prop_assert_eq!(&decoded, &checkpoint);
            prop_assert!(rest.is_empty());

            legacy_buf.extend_from_slice(&TRAILING);
            let (decoded, rest) = MerkleCheckpoint::from_compact(&legacy_buf, legacy_len);
            prop_assert_eq!(decoded, checkpoint);
            prop_assert_eq!(rest, &TRAILING[..]);

            let (decoded, _) = LegacyMerkleCheckpoint::from_compact(&buf, len);
            prop_assert_eq!(decoded, legacy);
        }
    }

    #[cfg(feature = "account-ext")]
    proptest! {
        #[test]
        fn storage_root_merkle_checkpoint_extension_roundtrip(
            legacy in legacy_storage_root_checkpoint(),
            extension in vec(any::<u8>(), 0..=64),
        ) {
            let mut expected = Vec::new();
            legacy.to_compact(&mut expected);

            let checkpoint = StorageRootMerkleCheckpoint {
                account_extension: extension.clone().into(),
                ..StorageRootMerkleCheckpoint::from(legacy)
            };
            let mut buf = Vec::new();
            let len = checkpoint.to_compact(&mut buf);
            prop_assert_eq!(len, buf.len());

            // A nonempty extension is appended to the legacy layout behind a length prefix.
            if !extension.is_empty() {
                expected.extend_from_slice(&(extension.len() as u16).to_be_bytes());
                expected.extend_from_slice(&extension);
            }
            prop_assert_eq!(&buf, &expected);

            buf.extend_from_slice(&TRAILING);
            let (decoded, rest) = StorageRootMerkleCheckpoint::from_compact(&buf, len);
            prop_assert_eq!(&decoded, &checkpoint);
            prop_assert_eq!(rest, &TRAILING[..]);

            let merkle = MerkleCheckpoint {
                storage_root_checkpoint: Some(checkpoint),
                ..Default::default()
            };
            let mut buf = Vec::new();
            merkle.to_compact(&mut buf);
            let (decoded, _) = MerkleCheckpoint::from_compact(&buf, buf.len());
            prop_assert_eq!(decoded, merkle);
        }
    }
}
