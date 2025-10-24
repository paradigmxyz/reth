use crate::{
    user::MerkleChangeSets, AccountHistory, SenderRecovery, StorageHistory, TransactionLookup,
    UserReceipts,
};
use alloy_eips::Encodable2718;
use reth_db_api::{table::Value, transaction::DbTxMut};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::StaticFileProvider, BlockReader, ChainStateBlockReader, DBProvider,
    PruneCheckpointReader, PruneCheckpointWriter, StaticFileProviderFactory,
};
use reth_prune::segments::SegmentSet;
use reth_prune_types::PruneModes;

/// Creates a [`SegmentSet`] from an existing components, such as [`StaticFileProvider`] and
/// [`PruneModes`].
pub fn from_components<Provider>(
    _static_file_provider: StaticFileProvider<Provider::Primitives>,
    prune_modes: PruneModes,
) -> SegmentSet<Provider>
where
    Provider: StaticFileProviderFactory<
            Primitives: NodePrimitives<SignedTx: Value, Receipt: Value, BlockHeader: Value>,
        > + DBProvider<Tx: DbTxMut>
        + ChainStateBlockReader
        + PruneCheckpointWriter
        + PruneCheckpointReader
        + BlockReader<Transaction: Encodable2718>,
{
    let PruneModes {
        sender_recovery,
        transaction_lookup,
        receipts,
        account_history,
        storage_history,
        bodies_history: _,
        merkle_changesets,
        receipts_log_filter: _,
    } = prune_modes;

    SegmentSet::default()
        // Merkle changesets
        .segment(MerkleChangeSets::new(merkle_changesets))
        // Account history
        .segment_opt(account_history.map(AccountHistory::new))
        // Storage history
        .segment_opt(storage_history.map(StorageHistory::new))
        // User receipts
        .segment_opt(receipts.map(UserReceipts::new))
        // Transaction lookup
        .segment_opt(transaction_lookup.map(TransactionLookup::new))
        // Sender recovery
        .segment_opt(sender_recovery.map(SenderRecovery::new))
}
