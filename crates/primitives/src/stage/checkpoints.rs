use crate::{
    trie::{hash_builder::HashBuilderState, StoredSubNode},
    Address, BlockNumber, B256,
};
use bytes::{Buf, BufMut};
use reth_codecs::{derive_arbitrary, main_codec, Compact};
use serde::{Deserialize, Serialize};
use std::{
    fmt::{Display, Formatter},
    ops::RangeInclusive,
};

/// Saves the progress of Merkle stage.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct MerkleCheckpoint {
    /// The target block number.
    pub target_block: BlockNumber,
    /// The last hashed account key processed.
    pub last_account_key: B256,
    /// The last walker key processed.
    pub last_walker_key: Vec<u8>,
    /// Previously recorded walker stack.
    pub walker_stack: Vec<StoredSubNode>,
    /// The hash builder state.
    pub state: HashBuilderState,
}

impl MerkleCheckpoint {
    /// Creates a new Merkle checkpoint.
    pub fn new(
        target_block: BlockNumber,
        last_account_key: B256,
        last_walker_key: Vec<u8>,
        walker_stack: Vec<StoredSubNode>,
        state: HashBuilderState,
    ) -> Self {
        Self { target_block, last_account_key, last_walker_key, walker_stack, state }
    }
}

impl Compact for MerkleCheckpoint {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let mut len = 0;

        buf.put_u64(self.target_block);
        len += 8;

        buf.put_slice(self.last_account_key.as_slice());
        len += self.last_account_key.len();

        buf.put_u16(self.last_walker_key.len() as u16);
        buf.put_slice(&self.last_walker_key[..]);
        len += 2 + self.last_walker_key.len();

        buf.put_u16(self.walker_stack.len() as u16);
        len += 2;
        for item in self.walker_stack.into_iter() {
            len += item.to_compact(buf);
        }

        len += self.state.to_compact(buf);
        len
    }

    fn from_compact(mut buf: &[u8], _len: usize) -> (Self, &[u8])
    where
        Self: Sized,
    {
        let target_block = buf.get_u64();

        let last_account_key = B256::from_slice(&buf[..32]);
        buf.advance(32);

        let last_walker_key_len = buf.get_u16() as usize;
        let last_walker_key = Vec::from(&buf[..last_walker_key_len]);
        buf.advance(last_walker_key_len);

        let walker_stack_len = buf.get_u16() as usize;
        let mut walker_stack = Vec::with_capacity(walker_stack_len);
        for _ in 0..walker_stack_len {
            let (item, rest) = StoredSubNode::from_compact(buf, 0);
            walker_stack.push(item);
            buf = rest;
        }

        let (state, buf) = HashBuilderState::from_compact(buf, 0);
        (
            MerkleCheckpoint {
                target_block,
                last_account_key,
                last_walker_key,
                walker_stack,
                state,
            },
            buf,
        )
    }
}

/// Saves the progress of AccountHashing stage.
#[main_codec]
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub struct AccountHashingCheckpoint {
    /// The next account to start hashing from.
    pub address: Option<Address>,
    /// Block range which this checkpoint is valid for.
    pub block_range: CheckpointBlockRange,
    /// Progress measured in accounts.
    pub progress: EntitiesCheckpoint,
}

/// Saves the progress of StorageHashing stage.
#[main_codec]
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
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
#[main_codec]
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub struct ExecutionCheckpoint {
    /// Block range which this checkpoint is valid for.
    pub block_range: CheckpointBlockRange,
    /// Progress measured in gas.
    pub progress: EntitiesCheckpoint,
}

/// Saves the progress of Headers stage.
#[main_codec]
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub struct HeadersCheckpoint {
    /// Block range which this checkpoint is valid for.
    pub block_range: CheckpointBlockRange,
    /// Progress measured in gas.
    pub progress: EntitiesCheckpoint,
}

/// Saves the progress of Index History stages.
#[main_codec]
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub struct IndexHistoryCheckpoint {
    /// Block range which this checkpoint is valid for.
    pub block_range: CheckpointBlockRange,
    /// Progress measured in changesets.
    pub progress: EntitiesCheckpoint,
}

/// Saves the progress of abstract stage iterating over or downloading entities.
#[main_codec]
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct EntitiesCheckpoint {
    /// Number of entities already processed.
    pub processed: u64,
    /// Total entities to be processed.
    pub total: u64,
}

impl Display for EntitiesCheckpoint {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.1}%", 100.0 * self.processed as f64 / self.total as f64)
    }
}

/// Saves the block range. Usually, it's used to check the validity of some stage checkpoint across
/// multiple executions.
#[main_codec]
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
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
#[main_codec]
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
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
    pub fn with_block_number(mut self, block_number: BlockNumber) -> Self {
        self.block_number = block_number;
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

impl Display for StageCheckpoint {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self.entities() {
            Some(entities) => entities.fmt(f),
            None => write!(f, "{}", self.block_number),
        }
    }
}

// TODO(alexey): add a merkle checkpoint. Currently it's hard because [`MerkleCheckpoint`]
//  is not a Copy type.
/// Stage-specific checkpoint metrics.
#[derive_arbitrary(compact)]
#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
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

/// Generates:
/// 1. [Compact::to_compact] and [Compact::from_compact] implementations for [StageUnitCheckpoint].
/// 2. [StageCheckpoint] getter and builder methods.
macro_rules! stage_unit_checkpoints {
    ($(($index:expr,$enum_variant:tt,$checkpoint_ty:ty,#[doc = $fn_get_doc:expr]$fn_get_name:ident,#[doc = $fn_build_doc:expr]$fn_build_name:ident)),+) => {
        impl Compact for StageUnitCheckpoint {
            fn to_compact<B>(self, buf: &mut B) -> usize
            where
                B: BufMut + AsMut<[u8]>,
            {
                match self {
                    $(
                        StageUnitCheckpoint::$enum_variant(data) => {
                            buf.put_u8($index);
                            1 + data.to_compact(buf)
                        }
                    )+
                }
            }

            fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8])
            where
                Self: Sized,
            {
                match buf[0] {
                    $(
                        $index => {
                            let (data, buf) = <$checkpoint_ty>::from_compact(&buf[1..], buf.len() - 1);
                            (Self::$enum_variant(data), buf)
                        }
                    )+
                    _ => unreachable!("Junk data in database: unknown StageUnitCheckpoint variant"),
                }
            }
        }

        impl StageCheckpoint {
            $(
                #[doc = $fn_get_doc]
                pub fn $fn_get_name(&self) -> Option<$checkpoint_ty> {
                    match self.stage_checkpoint {
                        Some(StageUnitCheckpoint::$enum_variant(checkpoint)) => Some(checkpoint),
                        _ => None,
                    }
                }

                #[doc = $fn_build_doc]
                pub fn $fn_build_name(
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

    #[test]
    fn merkle_checkpoint_roundtrip() {
        let mut rng = rand::thread_rng();
        let checkpoint = MerkleCheckpoint {
            target_block: rng.gen(),
            last_account_key: rng.gen(),
            last_walker_key: B256::random_with(&mut rng).to_vec(),
            walker_stack: vec![StoredSubNode {
                key: B256::random_with(&mut rng).to_vec(),
                nibble: Some(rng.gen()),
                node: None,
            }],
            state: HashBuilderState::default(),
        };

        let mut buf = Vec::new();
        let encoded = checkpoint.clone().to_compact(&mut buf);
        let (decoded, _) = MerkleCheckpoint::from_compact(&buf, encoded);
        assert_eq!(decoded, checkpoint);
    }

    #[test]
    fn stage_unit_checkpoint_roundtrip() {
        let mut rng = rand::thread_rng();
        let checkpoints = vec![
            StageUnitCheckpoint::Account(AccountHashingCheckpoint {
                address: Some(rng.gen()),
                block_range: CheckpointBlockRange { from: rng.gen(), to: rng.gen() },
                progress: EntitiesCheckpoint {
                    processed: rng.gen::<u32>() as u64,
                    total: u32::MAX as u64 + rng.gen::<u64>(),
                },
            }),
            StageUnitCheckpoint::Storage(StorageHashingCheckpoint {
                address: Some(rng.gen()),
                storage: Some(rng.gen()),
                block_range: CheckpointBlockRange { from: rng.gen(), to: rng.gen() },
                progress: EntitiesCheckpoint {
                    processed: rng.gen::<u32>() as u64,
                    total: u32::MAX as u64 + rng.gen::<u64>(),
                },
            }),
            StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                processed: rng.gen::<u32>() as u64,
                total: u32::MAX as u64 + rng.gen::<u64>(),
            }),
            StageUnitCheckpoint::Execution(ExecutionCheckpoint {
                block_range: CheckpointBlockRange { from: rng.gen(), to: rng.gen() },
                progress: EntitiesCheckpoint {
                    processed: rng.gen::<u32>() as u64,
                    total: u32::MAX as u64 + rng.gen::<u64>(),
                },
            }),
            StageUnitCheckpoint::Headers(HeadersCheckpoint {
                block_range: CheckpointBlockRange { from: rng.gen(), to: rng.gen() },
                progress: EntitiesCheckpoint {
                    processed: rng.gen::<u32>() as u64,
                    total: u32::MAX as u64 + rng.gen::<u64>(),
                },
            }),
        ];

        for checkpoint in checkpoints {
            let mut buf = Vec::new();
            let encoded = checkpoint.to_compact(&mut buf);
            let (decoded, _) = StageUnitCheckpoint::from_compact(&buf, encoded);
            assert_eq!(decoded, checkpoint);
        }
    }
}
