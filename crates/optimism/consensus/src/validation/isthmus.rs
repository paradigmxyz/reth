//! Block verification w.r.t. consensus rules new in Isthmus hardfork.

use alloy_consensus::Header;
use reth_optimism_primitives::predeploys::ADDRESS_L2_TO_L1_MESSAGE_PASSER;
use reth_storage_api::StorageRootProvider;
use reth_trie::{test_utils::storage_root_prehashed, HashedStorage};
use revm::db::BundleState;

use crate::OpConsensusError;

/// Verifies that `withdrawals_root` (i.e. `l2tol1-msg-passer` storage root since Isthmus) field is
/// set in block header.
pub fn verify_withdrawals_storage_root_is_some(header: &Header) -> Result<(), OpConsensusError> {
    header.withdrawals_root.as_ref().ok_or(OpConsensusError::StorageRootMissing)?;

    Ok(())
}

/// Verifies block header field `withdrawals_root` against storage root of
/// `l2tol1-message-passer` predeploy post block execution.
pub fn verify_withdrawals_storage_root<DB: StorageRootProvider>(
    state_updates: &BundleState,
    state: DB,
    header: &Header,
) -> Result<(), OpConsensusError> {
    let header_storage_root =
        header.withdrawals_root.expect("should be dropped in pre-exec verification");

    let storage_root = match state_updates.state().get(&ADDRESS_L2_TO_L1_MESSAGE_PASSER) {
        Some(account) => {
            // block contained withdrawals transactions, use predeploy storage updates from
            // execution
            let hashed_storage = HashedStorage::from_plain_storage(
                account.status,
                account.storage.iter().map(|(slot, value)| (slot, &value.present_value)),
            );
            storage_root_prehashed(hashed_storage.storage)
        }
        None => {
            // no withdrawals transactions in block, load latest storage root of predeploy
            // todo: reorg safe way to cache latest storage root?
            state
                .storage_root(ADDRESS_L2_TO_L1_MESSAGE_PASSER, Default::default())
                .map_err(OpConsensusError::StorageRootCalculationFail)?
        }
    };

    if header_storage_root != storage_root {
        return Err(OpConsensusError::StorageRootMismatch {
            header: header_storage_root,
            exec_res: storage_root,
        })
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use core::str::FromStr;
    use std::sync::Arc;

    use alloy_chains::Chain;
    use alloy_consensus::Header;
    use alloy_primitives::{keccak256, B256, U256};
    use reth_db_common::init::init_genesis;
    use reth_optimism_chainspec::OpChainSpecBuilder;
    use reth_optimism_node::OpNode;
    use reth_provider::{
        providers::BlockchainProvider2, test_utils::create_test_provider_factory_with_node_types,
        StateWriter,
    };
    use reth_storage_api::StateProviderFactory;
    use reth_trie::{test_utils::storage_root_prehashed, HashedPostState};

    use super::*;

    #[test]
    fn l2tol1_message_passer_no_withdrawals() {
        let hashed_address = keccak256(ADDRESS_L2_TO_L1_MESSAGE_PASSER);

        // create account storage
        let init_storage = HashedStorage::from_iter(
            false,
            [
                "50000000000000000000000000000004253371b55351a08cb3267d4d265530b6",
                "512428ed685fff57294d1a9cbb147b18ae5db9cf6ae4b312fa1946ba0561882e",
                "51e6784c736ef8548f856909870b38e49ef7a4e3e77e5e945e0d5e6fcaa3037f",
            ]
            .into_iter()
            .map(|str| (B256::from_str(str).unwrap(), U256::from(1))),
        );
        let mut state = HashedPostState::default();
        state.storages.insert(hashed_address, init_storage.clone());

        // init test db
        // note: must be empty (default) chain spec to ensure storage is empty after init genesis,
        // otherwise can't use `storage_root_prehashed` to determine storage root later
        let provider_factory = create_test_provider_factory_with_node_types::<OpNode>(Arc::new(
            OpChainSpecBuilder::default().chain(Chain::dev()).genesis(Default::default()).build(),
        ));
        let _ = init_genesis(&provider_factory).unwrap();

        // write account storage to database
        let provider_rw = provider_factory.provider_rw().unwrap();
        provider_rw.write_hashed_state(&state.clone().into_sorted()).unwrap();
        provider_rw.commit().unwrap();

        // create block header with withdrawals root set to storage root of l2tol1-msg-passer
        let header = Header {
            withdrawals_root: Some(storage_root_prehashed(init_storage.storage)),
            ..Default::default()
        };

        // create state provider factory
        let state_provider_factory = BlockchainProvider2::new(provider_factory).unwrap();

        // validate block against existing state by passing empty state updates
        let block_execution_state_updates = BundleState::default();
        verify_withdrawals_storage_root(
            &block_execution_state_updates,
            state_provider_factory.latest().expect("load state"),
            &header,
        )
        .unwrap();
    }
}
