use alloc::vec::Vec;
use alloy_primitives::{keccak256, Bytes, B256};
use reth_trie::{ExecutionWitnessMode, HashedPostState};
use revm::database::State;

/// Borrows finalized execution state for witness generation.
#[derive(Debug, Clone)]
pub struct ExecutionWitnessRecord<'a, DB> {
    /// State after execution.
    state: &'a State<DB>,
    /// Additional hashed state to include in the witness.
    additional_state: Option<HashedPostState>,
}

impl<'a, DB> ExecutionWitnessRecord<'a, DB> {
    /// Creates a new record from the state after execution.
    pub const fn new(state: &'a State<DB>) -> Self {
        Self { state, additional_state: None }
    }

    /// Adds hashed state that should be included when generating the witness.
    ///
    /// State recorded during execution takes precedence over additional state for overlapping
    /// accounts and storage slots.
    pub fn with_additional_state(mut self, additional_state: HashedPostState) -> Self {
        self.additional_state.get_or_insert_default().extend(additional_state);
        self
    }

    /// Converts this record into a complete [`alloy_rpc_types_debug::ExecutionWitness`] by
    /// generating state proofs and fetching ancestor block headers.
    ///
    /// The `block_number` is the number of the block being witnessed. Ancestor headers are
    /// included based on the lowest block number referenced by BLOCKHASH opcodes during
    /// execution, or just the parent header if BLOCKHASH was not called.
    #[cfg(feature = "witness")]
    pub fn into_execution_witness<SP, HP>(
        self,
        state_provider: &SP,
        headers_provider: &HP,
        block_number: u64,
        mode: ExecutionWitnessMode,
    ) -> reth_storage_errors::provider::ProviderResult<alloy_rpc_types_debug::ExecutionWitness>
    where
        SP: reth_storage_api::HashedPostStateProvider
            + reth_storage_api::StateProofProvider
            + ?Sized,
        HP: reth_storage_api::HeaderProvider + ?Sized,
        HP::Header: alloy_rlp::Encodable,
    {
        let codes = match mode {
            ExecutionWitnessMode::Legacy => self
                .state
                .cache
                .contracts
                .values()
                .map(|code| code.original_bytes())
                .chain(
                    // cache state does not have all the contracts, especially when
                    // a contract is created within the block
                    // the contract only exists in bundle state, therefore we need
                    // to include them as well
                    self.state.bundle_state.contracts.values().map(|code| code.original_bytes()),
                )
                .collect(),
            ExecutionWitnessMode::Canonical => {
                let mut codes: Vec<_> = self
                    .state
                    .cache
                    .contracts
                    .values()
                    .map(|c| c.original_bytes())
                    .filter(|code| !code.is_empty())
                    .collect();
                codes.sort_unstable();
                codes
            }
        };

        let lowest_block_number =
            self.state.block_hashes.lowest().map(|(block_number, _)| block_number);
        let (hashed_state, keys) = self.hashed_post_state(state_provider)?;

        let state = state_provider.witness(Default::default(), hashed_state, mode)?;
        let mut exec_witness =
            alloy_rpc_types_debug::ExecutionWitness { state, codes, keys, ..Default::default() };

        let smallest = lowest_block_number.unwrap_or_else(|| block_number.saturating_sub(1));
        let range = smallest..block_number;

        exec_witness.headers = headers_provider
            .headers_range(range)?
            .into_iter()
            .map(|header| {
                let mut buf = Vec::new();
                alloy_rlp::Encodable::encode(&header, &mut buf);
                buf.into()
            })
            .collect();

        Ok(exec_witness)
    }

    #[cfg(feature = "witness")]
    fn hashed_post_state<SP>(
        self,
        state_provider: &SP,
    ) -> reth_storage_errors::provider::ProviderResult<(HashedPostState, Vec<Bytes>)>
    where
        SP: reth_storage_api::HashedPostStateProvider + ?Sized,
    {
        let mut hashed_state = self.additional_state.unwrap_or_default();
        let mut keys = Vec::new();
        for (address, account) in &self.state.cache.accounts {
            let hashed_address = keccak256(address);
            hashed_state
                .accounts
                .insert(hashed_address, account.account.as_ref().map(|a| (&a.info).into()));

            let storage = hashed_state.storages.entry(hashed_address).or_default();

            if let Some(account) = &account.account {
                keys.push(address.to_vec().into());

                for (slot, value) in &account.storage {
                    let slot = B256::from(*slot);
                    let hashed_slot = keccak256(slot);
                    storage.storage.insert(hashed_slot, *value);

                    keys.push(slot.into());
                }
            }
        }

        // The execution cache does not contain untouched slots of a destroyed account. The
        // provider expands them into explicit zero writes from the parent state; extending it last
        // also ensures the bundle's final values override those collected from the cache.
        hashed_state.extend(state_provider.hashed_post_state(&self.state.bundle_state)?);
        Ok((hashed_state, keys))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, U256};
    use reth_storage_api::HashedPostStateProvider;
    use reth_storage_errors::provider::ProviderResult;
    use reth_trie::HashedStorage;
    use revm::{
        database::{states::CacheAccount, AccountStatus, BundleAccount, EmptyDB},
        state::AccountInfo,
    };

    #[derive(Debug)]
    struct ExpandedStateProvider(HashedPostState);

    impl HashedPostStateProvider for ExpandedStateProvider {
        fn hashed_post_state(
            &self,
            bundle_state: &revm::database::BundleState,
        ) -> ProviderResult<HashedPostState> {
            assert!(bundle_state.state.values().any(BundleAccount::was_destroyed));
            Ok(self.0.clone())
        }
    }

    #[derive(Debug)]
    struct StaticStateProvider(HashedPostState);

    impl HashedPostStateProvider for StaticStateProvider {
        fn hashed_post_state(
            &self,
            _bundle_state: &revm::database::BundleState,
        ) -> ProviderResult<HashedPostState> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn destroyed_account_storage_is_zero_expanded() {
        let address = Address::with_last_byte(1);
        let hashed_address = keccak256(address);
        let hashed_slot = B256::with_last_byte(2);

        let mut state = State::builder().with_database(EmptyDB::default()).build();
        state.cache.accounts.insert(address, CacheAccount::new_destroyed());
        state.bundle_state.state.insert(
            address,
            BundleAccount::new(
                Some(AccountInfo::default()),
                None,
                Default::default(),
                AccountStatus::Destroyed,
            ),
        );

        let provider = ExpandedStateProvider(
            HashedPostState::default().with_accounts([(hashed_address, None)]).with_storages([(
                hashed_address,
                HashedStorage::from_iter([(hashed_slot, U256::ZERO)]),
            )]),
        );

        let (hashed_state, _) =
            ExecutionWitnessRecord::new(&state).hashed_post_state(&provider).unwrap();
        let storage = hashed_state.storages.get(&hashed_address).unwrap();
        assert_eq!(storage.storage.get(&hashed_slot), Some(&U256::ZERO));
    }

    #[test]
    fn additional_state_is_merged_with_executed_state() {
        let address = Address::with_last_byte(1);
        let hashed_address = keccak256(address);
        let slot = U256::from(1);
        let additional_slot = B256::with_last_byte(2);

        let mut state = State::builder().with_database(EmptyDB::default()).build();
        let account = CacheAccount::new_loaded(
            AccountInfo::default(),
            core::iter::once((slot, U256::from(2))).collect(),
        );
        state.cache.accounts.insert(address, account);

        let additional_state = HashedPostState::default().with_storages([(
            hashed_address,
            HashedStorage::from_iter([
                (keccak256(B256::from(slot)), U256::from(1)),
                (additional_slot, U256::from(3)),
            ]),
        )]);
        let provider = StaticStateProvider(HashedPostState::default());

        let (hashed_state, _) = ExecutionWitnessRecord::new(&state)
            .with_additional_state(additional_state)
            .hashed_post_state(&provider)
            .unwrap();
        let storage = &hashed_state.storages[&hashed_address].storage;
        assert_eq!(storage[&keccak256(B256::from(slot))], U256::from(2));
        assert_eq!(storage[&additional_slot], U256::from(3));
    }
}
