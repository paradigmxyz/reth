//! Standalone crate for Optimism-Storage Reth.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
// The `optimism` feature must be enabled to use this crate.
#![cfg(feature = "optimism")]

#[cfg(test)]
mod tests {
    use reth_codecs::{test_utils::UnusedBits, validate_bitflag_backwards_compat};
    use reth_db_api::models::{
        CompactClientVersion, CompactU256, CompactU64, StoredBlockBodyIndices, StoredBlockOmmers,
        StoredBlockWithdrawals,
    };
    use reth_primitives::{
        Account, Receipt, ReceiptWithBloom, Requests, SealedHeader, Withdrawals,
    };
    use reth_prune_types::{PruneCheckpoint, PruneMode, PruneSegment};
    use reth_stages_types::{
        AccountHashingCheckpoint, CheckpointBlockRange, EntitiesCheckpoint, ExecutionCheckpoint,
        HeadersCheckpoint, IndexHistoryCheckpoint, StageCheckpoint, StageUnitCheckpoint,
        StorageHashingCheckpoint,
    };

    #[test]
    fn test_ensure_backwards_compatibility() {
        assert_eq!(Account::BITFLAG_ENCODED_BYTES, 2);
        assert_eq!(AccountHashingCheckpoint::BITFLAG_ENCODED_BYTES, 1);
        assert_eq!(CheckpointBlockRange::BITFLAG_ENCODED_BYTES, 1);
        assert_eq!(CompactClientVersion::BITFLAG_ENCODED_BYTES, 0);
        assert_eq!(CompactU256::BITFLAG_ENCODED_BYTES, 1);
        assert_eq!(CompactU64::BITFLAG_ENCODED_BYTES, 1);
        assert_eq!(EntitiesCheckpoint::BITFLAG_ENCODED_BYTES, 1);
        assert_eq!(ExecutionCheckpoint::BITFLAG_ENCODED_BYTES, 0);
        assert_eq!(HeadersCheckpoint::BITFLAG_ENCODED_BYTES, 0);
        assert_eq!(IndexHistoryCheckpoint::BITFLAG_ENCODED_BYTES, 0);
        assert_eq!(PruneCheckpoint::BITFLAG_ENCODED_BYTES, 1);
        assert_eq!(PruneMode::BITFLAG_ENCODED_BYTES, 1);
        assert_eq!(PruneSegment::BITFLAG_ENCODED_BYTES, 1);
        assert_eq!(Receipt::BITFLAG_ENCODED_BYTES, 2);
        assert_eq!(ReceiptWithBloom::BITFLAG_ENCODED_BYTES, 0);
        assert_eq!(SealedHeader::BITFLAG_ENCODED_BYTES, 0);
        assert_eq!(StageCheckpoint::BITFLAG_ENCODED_BYTES, 1);
        assert_eq!(StageUnitCheckpoint::BITFLAG_ENCODED_BYTES, 1);
        assert_eq!(StoredBlockBodyIndices::BITFLAG_ENCODED_BYTES, 1);
        assert_eq!(StoredBlockOmmers::BITFLAG_ENCODED_BYTES, 0);
        assert_eq!(StoredBlockWithdrawals::BITFLAG_ENCODED_BYTES, 0);
        assert_eq!(StorageHashingCheckpoint::BITFLAG_ENCODED_BYTES, 1);
        assert_eq!(Withdrawals::BITFLAG_ENCODED_BYTES, 0);

        // In case of failure, refer to the documentation of the
        // [`validate_bitflag_backwards_compat`] macro for detailed instructions on handling
        // it.
        validate_bitflag_backwards_compat!(Account, UnusedBits::NotZero);
        validate_bitflag_backwards_compat!(AccountHashingCheckpoint, UnusedBits::NotZero);
        validate_bitflag_backwards_compat!(CheckpointBlockRange, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(CompactClientVersion, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(CompactU256, UnusedBits::NotZero);
        validate_bitflag_backwards_compat!(CompactU64, UnusedBits::NotZero);
        validate_bitflag_backwards_compat!(EntitiesCheckpoint, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(ExecutionCheckpoint, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(HeadersCheckpoint, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(IndexHistoryCheckpoint, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(PruneCheckpoint, UnusedBits::NotZero);
        validate_bitflag_backwards_compat!(PruneMode, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(PruneSegment, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(Receipt, UnusedBits::NotZero);
        validate_bitflag_backwards_compat!(ReceiptWithBloom, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(SealedHeader, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(StageCheckpoint, UnusedBits::NotZero);
        validate_bitflag_backwards_compat!(StageUnitCheckpoint, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(StoredBlockBodyIndices, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(StoredBlockOmmers, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(StoredBlockWithdrawals, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(StorageHashingCheckpoint, UnusedBits::NotZero);
        validate_bitflag_backwards_compat!(Withdrawals, UnusedBits::Zero);
        validate_bitflag_backwards_compat!(Requests, UnusedBits::Zero);
    }
}
