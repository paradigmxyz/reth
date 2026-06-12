use alloc::vec::Vec;
#[cfg(feature = "std")]
use std::sync::OnceLock;

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
    #[deprecated(
        note = "MerkleChangeSets stage has been removed; kept for DB checkpoint compatibility"
    )]
    MerkleChangeSets,
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
    Prune,
    Finish,
    /// Other custom stage with a provided string identifier.
    Other(&'static str),
}

/// One-time-allocated stage ids encoded as raw Vecs, useful for database
/// clients to reference them for queries instead of encoding anew per query.
#[cfg(feature = "std")]
static ENCODED_STAGE_IDS: OnceLock<[Vec<u8>; StageId::ALL.len()]> = OnceLock::new();

impl StageId {
    /// All supported Stages
    pub const ALL: [Self; 15] = [
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
            #[expect(deprecated)]
            Self::MerkleChangeSets => "MerkleChangeSets",
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

    const fn pre_encoded_index(&self) -> Option<usize> {
        match self {
            Self::Era => Some(0),
            Self::Headers => Some(1),
            Self::Bodies => Some(2),
            Self::SenderRecovery => Some(3),
            Self::Execution => Some(4),
            Self::PruneSenderRecovery => Some(5),
            Self::MerkleUnwind => Some(6),
            Self::AccountHashing => Some(7),
            Self::StorageHashing => Some(8),
            Self::MerkleExecute => Some(9),
            Self::TransactionLookup => Some(10),
            Self::IndexStorageHistory => Some(11),
            Self::IndexAccountHistory => Some(12),
            Self::Prune => Some(13),
            Self::Finish => Some(14),
            _ => None,
        }
    }

    /// Get a pre-encoded raw Vec, for example, to be used as the DB key for
    /// `tables::StageCheckpoints` and `tables::StageCheckpointProgresses`
    pub fn get_pre_encoded(&self) -> Option<&Vec<u8>> {
        let index = self.pre_encoded_index()?;

        #[cfg(not(feature = "std"))]
        {
            let _ = index;
            None
        }
        #[cfg(feature = "std")]
        {
            Some(&ENCODED_STAGE_IDS.get_or_init(|| Self::ALL.map(|id| id.as_str().into()))[index])
        }
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
    fn pre_encoded_stage_id_matches_string_key() {
        for stage_id in StageId::ALL {
            assert_eq!(
                stage_id.get_pre_encoded().map(Vec::as_slice),
                Some(stage_id.as_str().as_bytes())
            );
        }

        assert_eq!(StageId::Other("Foo").get_pre_encoded(), None);
    }

    #[test]
    fn is_downloading_stage() {
        assert!(StageId::Headers.is_downloading_stage());
        assert!(StageId::Bodies.is_downloading_stage());
        assert!(StageId::Era.is_downloading_stage());

        assert!(!StageId::Execution.is_downloading_stage());
    }
}
