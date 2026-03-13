//! [`StateProvider`](crate::StateProvider) implementations
pub(crate) mod historical;
pub(crate) mod latest;
pub(crate) mod macros;
pub(crate) mod overlay;

use crate::{ProviderError, ProviderResult};
use reth_db_api::{tables, transaction::DbTx};

pub(crate) fn has_hashed_snapshot_marker<TX: DbTx>(tx: &TX) -> ProviderResult<bool> {
    tx.get::<tables::StageCheckpointProgresses>(tables::HASHED_SNAPSHOT_MARKER.to_string())
        .map(|marker| marker.is_some())
        .map_err(ProviderError::Database)
}
