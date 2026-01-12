use alloc::vec::Vec;
#[cfg(feature = "std")]
use std::{collections::HashMap, sync::OnceLock};

/// Stage IDs for all known stages.
///
/// For custom stages, use [`StageId::Other`]
#[expect(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StageId {
    #[deprecated(
        note = "Static Files are generated outside of the pipeline and do not require a separate stage"
    )]
    StaticFile,
    Era,
    Headers,
    Bodies,
    SenderRecovery,
    Execution,
    PruneSenderRecovery,
    MerkleUnwind,
    AccountHashing,
    StorageHashing,
    MerkleExecute,
    TransactionLookup,
    IndexStorageHistory,
    IndexAccountHistory,
    MerkleChangeSets,
    Prune,
    Finish,
    /// Other custom stage with a provided string identifier.
    Other(&'static str),
}

/// One-time-allocated stage ids encoded as raw Vecs, useful for database
/// clients to reference them for queries instead of encoding anew per query
/// (sad heap allocation required).
#[cfg(feature = "std")]
static ENCODED_STAGE_IDS: OnceLock<HashMap<StageId, Vec<u8>>> = OnceLock::new();

impl StageId {
    /// All supported stages in the default pipeline execution order.
    ///
    /// NOTE: This is used in various places to iterate stage checkpoints/metrics. Keeping this in
    /// sync with the default stage set (see `crates/stages/stages/src/sets.rs`) avoids confusing
    /// ordering mismatches in output/metrics and reduces documentation drift.
    pub const ALL: [Self; 16] = [
        Self::Era,
        Self::Headers,
        Self::Bodies,
        Self::SenderRecovery,
        Self::Execution,
        Self::PruneSenderRecovery,
        Self::MerkleUnwind,
        Self::AccountHashing,
        Self::StorageHashing,
        Self::MerkleExecute,
        Self::MerkleChangeSets,
        Self::TransactionLookup,
        Self::IndexStorageHistory,
        Self::IndexAccountHistory,
        Self::Prune,
        Self::Finish,
    ];

    /// Stages that require state.
    pub const STATE_REQUIRED: [Self; 9] = [
        Self::Execution,
        Self::PruneSenderRecovery,
        Self::MerkleUnwind,
        Self::AccountHashing,
        Self::StorageHashing,
        Self::MerkleExecute,
        Self::IndexStorageHistory,
        Self::IndexAccountHistory,
        Self::Prune,
    ];

    /// Return stage id formatted as string.
    pub const fn as_str(&self) -> &str {
        match self {
            #[expect(deprecated)]
            Self::StaticFile => "StaticFile",
            Self::Era => "Era",
            Self::Headers => "Headers",
            Self::Bodies => "Bodies",
            Self::SenderRecovery => "SenderRecovery",
            Self::Execution => "Execution",
            Self::PruneSenderRecovery => "PruneSenderRecovery",
            Self::MerkleUnwind => "MerkleUnwind",
            Self::AccountHashing => "AccountHashing",
            Self::StorageHashing => "StorageHashing",
            Self::MerkleExecute => "MerkleExecute",
            Self::TransactionLookup => "TransactionLookup",
            Self::IndexAccountHistory => "IndexAccountHistory",
            Self::IndexStorageHistory => "IndexStorageHistory",
            Self::MerkleChangeSets => "MerkleChangeSets",
            Self::Prune => "Prune",
            Self::Finish => "Finish",
            Self::Other(s) => s,
        }
    }

    /// Returns true if it's a downloading stage [`StageId::Headers`] or [`StageId::Bodies`]
    pub const fn is_downloading_stage(&self) -> bool {
        matches!(self, Self::Era | Self::Headers | Self::Bodies)
    }

    /// Returns `true` if it's [`TransactionLookup`](StageId::TransactionLookup) stage.
    pub const fn is_tx_lookup(&self) -> bool {
        matches!(self, Self::TransactionLookup)
    }

    /// Returns true indicating if it's the finish stage [`StageId::Finish`]
    pub const fn is_finish(&self) -> bool {
        matches!(self, Self::Finish)
    }

    /// Get a pre-encoded raw Vec, for example, to be used as the DB key for
    /// `tables::StageCheckpoints` and `tables::StageCheckpointProgresses`
    pub fn get_pre_encoded(&self) -> Option<&Vec<u8>> {
        #[cfg(not(feature = "std"))]
        {
            None
        }
        #[cfg(feature = "std")]
        ENCODED_STAGE_IDS
            .get_or_init(|| {
                let mut map = HashMap::with_capacity(Self::ALL.len());
                for stage_id in Self::ALL {
                    map.insert(stage_id, stage_id.to_string().into_bytes());
                }
                map
            })
            .get(self)
    }
}

impl core::fmt::Display for StageId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(id: StageId) -> usize {
        StageId::ALL
            .iter()
            .position(|&x| x == id)
            .unwrap_or_else(|| panic!("StageId::{id:?} missing from StageId::ALL"))
    }

    #[test]
    fn stage_id_all_preserves_expected_relative_order() {
        // Merkle changesets must be produced right after trie execution and before any history
        // indexing that relies on finalized trie-related change sets.
        assert!(pos(StageId::MerkleExecute) < pos(StageId::MerkleChangeSets));
        assert!(pos(StageId::MerkleChangeSets) < pos(StageId::TransactionLookup));

        // Basic online ordering invariants.
        assert!(pos(StageId::Headers) < pos(StageId::Bodies));
        assert!(pos(StageId::Bodies) < pos(StageId::SenderRecovery));
        assert!(pos(StageId::SenderRecovery) < pos(StageId::Execution));

        // Finish should remain the last stage.
        assert_eq!(pos(StageId::Finish), StageId::ALL.len() - 1);
    }

    #[test]
    fn stage_id_as_string() {
        assert_eq!(StageId::Era.to_string(), "Era");
        assert_eq!(StageId::Headers.to_string(), "Headers");
        assert_eq!(StageId::Bodies.to_string(), "Bodies");
        assert_eq!(StageId::SenderRecovery.to_string(), "SenderRecovery");
        assert_eq!(StageId::Execution.to_string(), "Execution");
        assert_eq!(StageId::MerkleUnwind.to_string(), "MerkleUnwind");
        assert_eq!(StageId::AccountHashing.to_string(), "AccountHashing");
        assert_eq!(StageId::StorageHashing.to_string(), "StorageHashing");
        assert_eq!(StageId::MerkleExecute.to_string(), "MerkleExecute");
        assert_eq!(StageId::IndexAccountHistory.to_string(), "IndexAccountHistory");
        assert_eq!(StageId::IndexStorageHistory.to_string(), "IndexStorageHistory");
        assert_eq!(StageId::TransactionLookup.to_string(), "TransactionLookup");
        assert_eq!(StageId::Finish.to_string(), "Finish");

        assert_eq!(StageId::Other("Foo").to_string(), "Foo");
    }

    #[test]
    fn is_downloading_stage() {
        assert!(StageId::Headers.is_downloading_stage());
        assert!(StageId::Bodies.is_downloading_stage());
        assert!(StageId::Era.is_downloading_stage());

        assert!(!StageId::Execution.is_downloading_stage());
    }
}
