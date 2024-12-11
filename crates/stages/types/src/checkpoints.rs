use super::StageId;
use alloc::vec::Vec;
use alloy_primitives::{Address, BlockNumber, B256};
use core::ops::RangeInclusive;
use reth_trie_common::{hash_builder::HashBuilderState, StoredSubNode};
use serde::{Deserialize, Serialize};

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
}

impl MerkleCheckpoint {
    /// Creates a new Merkle checkpoint.
    pub const fn new(
        target_block: BlockNumber,
        last_account_key: B256,
        walker_stack: Vec<StoredSubNode>,
        state: HashBuilderState,
    ) -> Self {
        Self { target_block, last_account_key, walker_stack, state }
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

        let (state, buf) = HashBuilderState::from_compact(buf, 0);
        (Self { target_block, last_account_key, walker_stack, state }, buf)
    }
}

/// Saves the progress of AccountHashing stage.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
pub struct AccountHashingCheckpoint {
    /// The next account to start hashing from.
    pub address: Option<Address>,
    /// Block range which this checkpoint is valid for.
    pub block_range: CheckpointBlockRange,
    /// Progress measured in accounts.
    pub progress: EntitiesCheckpoint,
}

/// Saves the progress of StorageHashing stage.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
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
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
pub struct ExecutionCheckpoint {
    /// Block range which this checkpoint is valid for.
    pub block_range: CheckpointBlockRange,
    /// Progress measured in gas.
    pub progress: EntitiesCheckpoint,
}

/// Saves the progress of Headers stage.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
pub struct HeadersCheckpoint {
    /// Block range which this checkpoint is valid for.
    pub block_range: CheckpointBlockRange,
    /// Progress measured in gas.
    pub progress: EntitiesCheckpoint,
}

/// Saves the progress of Index History stages.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
pub struct IndexHistoryCheckpoint {
    /// Block range which this checkpoint is valid for.
    pub block_range: CheckpointBlockRange,
    /// Progress measured in changesets.
    pub progress: EntitiesCheckpoint,
}

/// Saves the progress of abstract stage iterating over or downloading entities.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
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
        Some(format!("{:.2}%", (percentage * 100.0).floor() / 100.0))
    }
}

/// Saves the block range. Usually, it's used to check the validity of some stage checkpoint across
/// multiple executions.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
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
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
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
        _ = self.stage_checkpoint.map(|mut checkpoint| checkpoint.set_block_range(from, to));
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
        }
    }
}

// TODO(alexey): add a merkle checkpoint. Currently it's hard because [`MerkleCheckpoint`]
//  is not a Copy type.
/// Stage-specific checkpoint metrics.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test-utils"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
pub enum StageUnitCheckpoint {
    /// Saves the progress of AccountHashing stage.
    Account(AccountHashingCheckpoint),
    /// Saves the progress of StorageHashing stage.
    Storage(StorageHashingCheckpoint),
    /// Saves the progress of abstract stage iterating over or downloading entities.
    Entities(EntitiesCheckpoint),
    /// Saves the progress of Execution stage.
    Execution(ExecutionCheckpoint),
    /// Saves the progress of Headers stage.
    Headers(HeadersCheckpoint),
    /// Saves the progress of Index History stage.
    IndexHistory(IndexHistoryCheckpoint),
}

impl StageUnitCheckpoint {
    /// Sets the block range. Returns old block range, or `None` if checkpoint doesn't use block
    /// range.
    pub fn set_block_range(&mut self, from: u64, to: u64) -> Option<CheckpointBlockRange> {
        match self {
            Self::Account(AccountHashingCheckpoint { ref mut block_range, .. }) |
            Self::Storage(StorageHashingCheckpoint { ref mut block_range, .. }) |
            Self::Execution(ExecutionCheckpoint { ref mut block_range, .. }) |
            Self::IndexHistory(IndexHistoryCheckpoint { ref mut block_range, .. }) => {
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
    )
);

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;
    use reth_codecs::Compact;

    #[test]
    fn merkle_checkpoint_roundtrip() {
        let mut rng = rand::thread_rng();
        let checkpoint = MerkleCheckpoint {
            target_block: rng.gen(),
            last_account_key: rng.gen(),
            walker_stack: vec![StoredSubNode {
                key: B256::random_with(&mut rng).to_vec(),
                nibble: Some(rng.gen()),
                node: None,
            }],
            state: HashBuilderState::default(),
        };

        let mut buf = Vec::new();
        let encoded = checkpoint.to_compact(&mut buf);
        let (decoded, _) = MerkleCheckpoint::from_compact(&buf, encoded);
        assert_eq!(decoded, checkpoint);
    }
}
