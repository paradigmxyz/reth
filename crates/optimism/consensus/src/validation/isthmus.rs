//! Block verification w.r.t. consensus rules new in Isthmus hardfork.

use crate::OpConsensusError;
use alloy_consensus::BlockHeader;
use core::fmt;
use reth_optimism_primitives::predeploys::ADDRESS_L2_TO_L1_MESSAGE_PASSER;
use reth_storage_api::StorageRootProvider;
use reth_trie_common::HashedStorage;
use revm::database::BundleAccount;

/// Verifies that `withdrawals_root` (i.e. `l2tol1-msg-passer` storage root since Isthmus) field is
/// set in block header.
pub fn ensure_withdrawals_storage_root_is_some<H: BlockHeader>(
    header: H,
) -> Result<(), OpConsensusError> {
    header.withdrawals_root().ok_or(OpConsensusError::L2WithdrawalsRootMissing)?;

    Ok(())
}

/// Verifies block header field `withdrawals_root` against storage root of
/// `L2ToL1MessagePasser.sol` predeploy post block execution.
///
/// See <https://specs.optimism.io/protocol/isthmus/exec-engine.html#l2tol1messagepasser-storage-root-in-header>.
pub fn verify_withdrawals_storage_root<DB, H>(
    predeploy_account_updates: Option<&BundleAccount>,
    state: DB,
    header: H,
) -> Result<(), OpConsensusError>
where
    DB: StorageRootProvider,
    H: BlockHeader + fmt::Debug,
{
    let header_storage_root =
        header.withdrawals_root().ok_or(OpConsensusError::L2WithdrawalsRootMissing)?;

    // if block contained l2 withdrawals transactions, use predeploy storage updates from
    // execution
    let hashed_storage_updates = predeploy_account_updates.map(|acc| {
        HashedStorage::from_plain_storage(
            acc.status,
            acc.storage.iter().map(|(slot, value)| (slot, &value.present_value)),
        )
    });

    let storage_root = state
        .storage_root(ADDRESS_L2_TO_L1_MESSAGE_PASSER, hashed_storage_updates.unwrap_or_default())
        .map_err(OpConsensusError::L2WithdrawalsRootCalculationFail)?;

    if header_storage_root != storage_root {
        return Err(OpConsensusError::L2WithdrawalsRootMismatch {
            header: header_storage_root,
            exec_res: storage_root,
        })
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use alloc::sync::Arc;
    use alloy_chains::Chain;
    use alloy_consensus::Header;
    use alloy_primitives::{keccak256, B256, U256};
    use core::str::FromStr;
    use reth_db_common::init::init_genesis;
    use reth_optimism_chainspec::OpChainSpecBuilder;
    use reth_optimism_node::OpNode;
    use reth_provider::{
        providers::BlockchainProvider, test_utils::create_test_provider_factory_with_node_types,
        StateWriter,
    };
    use reth_storage_api::StateProviderFactory;
    use reth_trie::test_utils::storage_root_prehashed;
    use reth_trie_common::HashedPostState;

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
        let state_provider_factory = BlockchainProvider::new(provider_factory).unwrap();

        // validate block against existing state by passing empty state updates
        verify_withdrawals_storage_root(
            None,
            state_provider_factory.latest().expect("load state"),
            &header,
        )
        .unwrap();
    }
}
