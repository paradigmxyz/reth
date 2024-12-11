//! Block validation w.r.t. consensus rules new in isthmus hardfork.

use reth_consensus::ConsensusError;
use reth_optimism_primitives::predeploys::ADDRESS_L2_TO_L1_MESSAGE_PASSER;
use reth_primitives::SealedHeader;
use reth_storage_api::{StateProviderFactory, StorageRootProvider};
use tracing::{debug, trace};

use crate::OpConsensusError;

/// Validates block header field `withdrawals_root` against storage root of
/// `L2-to-L1-message-passer` predeploy.
pub fn validate_l2_to_l1_msg_passer(
    provider: impl StateProviderFactory,
    header: &SealedHeader,
) -> Result<(), ConsensusError> {
    let state = provider.latest().map_err(|err| {
        debug!(target: "op::consensus",
            block_number=header.number,
            err=%OpConsensusError::LoadStorageRootFailed(err),
            "failed to load latest state",
        );

        ConsensusError::Other
    })?;

    let storage_root_msg_passer =
        state.storage_root(ADDRESS_L2_TO_L1_MESSAGE_PASSER, Default::default()).map_err(|err| {
            debug!(target: "op::consensus",
                block_number=header.number,
                err=%OpConsensusError::LoadStorageRootFailed(err),
                "failed to load storage root for l2tol1-msg-passer predeploy",
            );

            ConsensusError::Other
        })?;

    if header.withdrawals_root.is_none_or(|root| root != storage_root_msg_passer) {
        trace!(target: "op::consensus",
            block_number=header.number,
            err=%OpConsensusError::StorageRootMismatch {
                got: header.withdrawals_root,
                expected: storage_root_msg_passer
            },
            "block failed validation",
        );

        return Err(ConsensusError::Other)
    }

    Ok(())
}
