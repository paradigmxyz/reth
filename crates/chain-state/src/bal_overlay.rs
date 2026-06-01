use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_eip7928::{bal::Bal as AlloyBal, AccountChanges};
use alloy_primitives::{
    keccak256, map::HashMap, Address, Bytes, StorageKey, StorageValue, B256, U256,
};
use reth_errors::ProviderResult;
use reth_primitives_traits::{Account, Bytecode};
use reth_storage_api::{
    AccountReader, BlockHashReader, BytecodeReader, HashedPostStateProvider, StateProofProvider,
    StateProvider, StateRootProvider, StorageRootProvider,
};
use reth_trie::{
    updates::TrieUpdates, AccountProof, ExecutionWitnessMode, HashedPostState, HashedStorage,
    MultiProof, MultiProofTargets, StorageMultiProof, StorageProof, TrieInput,
};
use revm_bytecode::BytecodeDecodeError;
use revm_database::BundleState;

/// Final-write state overlay derived from a Block Access List.
///
/// This overlay stores only the BAL-declared final writes. Account, storage, and bytecode reads
/// that are not covered by the overlay fall back to the wrapped parent state provider.
#[derive(Clone, Debug, Default)]
pub struct BalStateOverlay {
    accounts: HashMap<Address, BalAccountOverlay>,
    bytecodes: HashMap<B256, Bytecode>,
}

impl BalStateOverlay {
    /// Builds a final-write overlay from an EIP-7928 BAL.
    pub fn from_bal(bal: &AlloyBal) -> Result<Self, BytecodeDecodeError> {
        Self::from_account_changes(bal.iter())
    }

    /// Builds a final-write overlay from decoded EIP-7928 account changes.
    pub fn from_account_changes<'a>(
        changes: impl IntoIterator<Item = &'a AccountChanges>,
    ) -> Result<Self, BytecodeDecodeError> {
        let mut overlay = Self::default();
        for account_changes in changes {
            overlay.apply_account_changes(account_changes)?;
        }
        Ok(overlay)
    }

    /// Applies decoded EIP-7928 account changes to this overlay.
    ///
    /// For each field, the last BAL write is the final post-parent value. Storage reads are ignored
    /// here because they are prefetch hints and must not shadow final writes or parent fallback
    /// reads.
    pub fn apply_account_changes(
        &mut self,
        account_changes: &AccountChanges,
    ) -> Result<(), BytecodeDecodeError> {
        let address = account_changes.address;

        if let Some(change) = account_changes.balance_changes.last() {
            self.set_balance(address, change.post_balance);
        }
        if let Some(change) = account_changes.nonce_changes.last() {
            self.set_nonce(address, change.new_nonce);
        }
        if let Some(change) = account_changes.code_changes.last() {
            self.set_code_bytes(address, change.new_code.clone())?;
        }
        for storage_changes in &account_changes.storage_changes {
            if let Some(change) = storage_changes.changes.last() {
                self.set_storage(address, StorageKey::from(storage_changes.slot), change.new_value);
            }
        }

        Ok(())
    }

    /// Returns `true` if the overlay has no final writes.
    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty() && self.bytecodes.is_empty()
    }

    /// Returns the number of accounts with final writes in this overlay.
    pub fn account_count(&self) -> usize {
        self.accounts.len()
    }

    /// Returns the number of storage slots with final writes in this overlay.
    pub fn storage_slot_count(&self) -> usize {
        self.accounts.values().map(|account| account.storage.len()).sum()
    }

    /// Returns the number of bytecode entries carried by this overlay.
    pub fn bytecode_count(&self) -> usize {
        self.bytecodes.len()
    }

    /// Deletes an account in the overlay.
    ///
    /// Storage reads for a deleted account return zero because the account's storage trie is wiped.
    pub fn delete_account(&mut self, address: Address) {
        self.account_entry(address).account = AccountOverlay::Deleted;
    }

    /// Sets the full account value in the overlay.
    pub fn set_account(&mut self, address: Address, account: Account) {
        self.account_entry(address).account = AccountOverlay::Full(account);
    }

    /// Sets the final balance for an account.
    pub fn set_balance(&mut self, address: Address, balance: U256) {
        self.account_entry(address).field_update().balance = Some(balance);
    }

    /// Sets the final nonce for an account.
    pub fn set_nonce(&mut self, address: Address, nonce: u64) {
        self.account_entry(address).field_update().nonce = Some(nonce);
    }

    /// Sets the final bytecode for an account from raw bytes and returns its code hash.
    pub fn set_code_bytes(
        &mut self,
        address: Address,
        bytecode: Bytes,
    ) -> Result<B256, BytecodeDecodeError> {
        if bytecode.is_empty() {
            self.set_empty_code(address);
            return Ok(KECCAK_EMPTY);
        }

        let hash = keccak256(&bytecode);
        let bytecode = Bytecode::new_raw_checked(bytecode)?;
        self.bytecodes.insert(hash, bytecode);
        self.account_entry(address).field_update().bytecode_hash = Some(Some(hash));
        Ok(hash)
    }

    /// Sets the account code hash to empty code.
    pub fn set_empty_code(&mut self, address: Address) {
        self.account_entry(address).field_update().bytecode_hash = Some(None);
    }

    /// Sets a final storage value for an account and slot.
    pub fn set_storage(&mut self, address: Address, slot: StorageKey, value: StorageValue) {
        self.account_entry(address).storage.insert(slot, value);
    }

    /// Wraps a fallback provider with this overlay.
    pub fn provider<S>(self, fallback: S) -> BalOverlayStateProvider<S> {
        BalOverlayStateProvider::new(fallback, self)
    }

    fn account_entry(&mut self, address: Address) -> &mut BalAccountOverlay {
        self.accounts.entry(address).or_default()
    }
}

/// State provider that composes a BAL final-write overlay over a fallback parent provider.
#[derive(Clone, Debug)]
pub struct BalOverlayStateProvider<S> {
    fallback: S,
    overlay: BalStateOverlay,
}

impl<S> BalOverlayStateProvider<S> {
    /// Creates a BAL overlay provider from a fallback provider and final-write overlay.
    pub const fn new(fallback: S, overlay: BalStateOverlay) -> Self {
        Self { fallback, overlay }
    }

    /// Returns the fallback provider.
    pub const fn fallback(&self) -> &S {
        &self.fallback
    }

    /// Returns the BAL final-write overlay.
    pub const fn overlay(&self) -> &BalStateOverlay {
        &self.overlay
    }

    /// Consumes this provider and returns its parts.
    pub fn into_parts(self) -> (S, BalStateOverlay) {
        (self.fallback, self.overlay)
    }
}

impl<S: StateProvider> BalOverlayStateProvider<S> {
    fn basic_account_with_overlay(&self, address: &Address) -> ProviderResult<Option<Account>> {
        let Some(account_overlay) = self.overlay.accounts.get(address) else {
            return self.fallback.basic_account(address);
        };

        match &account_overlay.account {
            AccountOverlay::Unchanged => self.fallback.basic_account(address),
            AccountOverlay::Deleted => Ok(None),
            AccountOverlay::Full(account) => Ok(Some(*account)),
            AccountOverlay::Update(fields) => {
                let base = self.fallback.basic_account(address)?;
                let mut account = base.unwrap_or_default();
                if let Some(balance) = fields.balance {
                    account.balance = balance;
                }
                if let Some(nonce) = fields.nonce {
                    account.nonce = nonce;
                }
                if let Some(bytecode_hash) = fields.bytecode_hash {
                    account.bytecode_hash = bytecode_hash;
                }
                Ok(Some(account))
            }
        }
    }

    /// Returns the hashed post-state represented by the BAL overlay over its fallback provider.
    ///
    /// Field-only account updates are materialized by reading the fallback account and applying
    /// the overlaid fields, so the returned state is suitable for comparing against trie data
    /// produced while validating the same BAL parent.
    pub fn hashed_overlay_state(&self) -> ProviderResult<HashedPostState> {
        let mut state = HashedPostState::with_capacity(self.overlay.accounts.len());

        for (address, account_overlay) in &self.overlay.accounts {
            let hashed_address = keccak256(address);

            match account_overlay.account {
                AccountOverlay::Unchanged => {}
                AccountOverlay::Deleted => {
                    state.accounts.insert(hashed_address, None);
                }
                AccountOverlay::Full(account) => {
                    state.accounts.insert(hashed_address, Some(account));
                }
                AccountOverlay::Update(_) => {
                    state
                        .accounts
                        .insert(hashed_address, self.basic_account_with_overlay(address)?);
                }
            }

            if matches!(account_overlay.account, AccountOverlay::Deleted) ||
                !account_overlay.storage.is_empty()
            {
                state.storages.insert(hashed_address, account_overlay.hashed_storage(address));
            }
        }

        Ok(state)
    }

    fn merged_hashed_storage(&self, address: Address, storage: HashedStorage) -> HashedStorage {
        let mut merged = self
            .overlay
            .accounts
            .get(&address)
            .map(|account| account.hashed_storage(&address))
            .unwrap_or_default();
        merged.extend(&storage);
        merged
    }

    fn input_with_overlay(&self, mut input: TrieInput) -> ProviderResult<TrieInput> {
        let overlay = self.hashed_overlay_state()?;
        if !overlay.is_empty() {
            input.prepend(overlay);
        }
        Ok(input)
    }
}

impl<S: StateProvider> BlockHashReader for BalOverlayStateProvider<S> {
    fn block_hash(&self, number: alloy_primitives::BlockNumber) -> ProviderResult<Option<B256>> {
        self.fallback.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: alloy_primitives::BlockNumber,
        end: alloy_primitives::BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.fallback.canonical_hashes_range(start, end)
    }
}

impl<S: StateProvider> AccountReader for BalOverlayStateProvider<S> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        self.basic_account_with_overlay(address)
    }
}

impl<S: StateProvider> BytecodeReader for BalOverlayStateProvider<S> {
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        if *code_hash == KECCAK_EMPTY {
            return Ok(None);
        }
        if let Some(bytecode) = self.overlay.bytecodes.get(code_hash) {
            return Ok(Some(bytecode.clone()));
        }
        self.fallback.bytecode_by_hash(code_hash)
    }
}

impl<S: StateProvider> StateRootProvider for BalOverlayStateProvider<S> {
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        self.state_root_from_nodes(TrieInput::from_state(hashed_state))
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        self.fallback.state_root_from_nodes(self.input_with_overlay(input)?)
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.state_root_from_nodes_with_updates(TrieInput::from_state(hashed_state))
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        self.fallback.state_root_from_nodes_with_updates(self.input_with_overlay(input)?)
    }
}

impl<S: StateProvider> StorageRootProvider for BalOverlayStateProvider<S> {
    fn storage_root(&self, address: Address, storage: HashedStorage) -> ProviderResult<B256> {
        self.fallback.storage_root(address, self.merged_hashed_storage(address, storage))
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        storage: HashedStorage,
    ) -> ProviderResult<StorageProof> {
        self.fallback.storage_proof(address, slot, self.merged_hashed_storage(address, storage))
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        self.fallback.storage_multiproof(
            address,
            slots,
            self.merged_hashed_storage(address, storage),
        )
    }
}

impl<S: StateProvider> StateProofProvider for BalOverlayStateProvider<S> {
    fn proof(
        &self,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        self.fallback.proof(self.input_with_overlay(input)?, address, slots)
    }

    fn multiproof(
        &self,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        self.fallback.multiproof(self.input_with_overlay(input)?, targets)
    }

    fn witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
        mode: ExecutionWitnessMode,
    ) -> ProviderResult<Vec<Bytes>> {
        self.fallback.witness(self.input_with_overlay(input)?, target, mode)
    }
}

impl<S: StateProvider> HashedPostStateProvider for BalOverlayStateProvider<S> {
    fn hashed_post_state(&self, bundle_state: &BundleState) -> HashedPostState {
        self.fallback.hashed_post_state(bundle_state)
    }
}

impl<S: StateProvider> StateProvider for BalOverlayStateProvider<S> {
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        let Some(account_overlay) = self.overlay.accounts.get(&account) else {
            return self.fallback.storage(account, storage_key);
        };

        if matches!(account_overlay.account, AccountOverlay::Deleted) {
            return Ok(Some(StorageValue::ZERO));
        }

        if let Some(value) = account_overlay.storage.get(&storage_key) {
            return Ok(Some(*value));
        }

        self.fallback.storage(account, storage_key)
    }
}

#[derive(Clone, Debug, Default)]
struct BalAccountOverlay {
    account: AccountOverlay,
    storage: HashMap<StorageKey, StorageValue>,
}

impl BalAccountOverlay {
    fn field_update(&mut self) -> &mut AccountFieldOverlay {
        if !matches!(self.account, AccountOverlay::Update(_)) {
            self.account = AccountOverlay::Update(AccountFieldOverlay::default());
        }

        let AccountOverlay::Update(fields) = &mut self.account else {
            unreachable!("account overlay just converted to field update")
        };
        fields
    }

    fn hashed_storage(&self, _address: &Address) -> HashedStorage {
        let wiped = matches!(self.account, AccountOverlay::Deleted);
        HashedStorage::from_iter(
            wiped,
            self.storage.iter().map(|(slot, value)| (keccak256(slot), *value)),
        )
    }
}

#[derive(Clone, Copy, Debug, Default)]
enum AccountOverlay {
    #[default]
    Unchanged,
    Deleted,
    Full(Account),
    Update(AccountFieldOverlay),
}

#[derive(Clone, Copy, Debug, Default)]
struct AccountFieldOverlay {
    balance: Option<U256>,
    nonce: Option<u64>,
    bytecode_hash: Option<Option<B256>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{
        BalanceChange, BlockAccessIndex, CodeChange, NonceChange, SlotChanges, StorageChange,
    };

    #[derive(Default)]
    struct MockProvider {
        accounts: HashMap<Address, Account>,
        storage: HashMap<(Address, StorageKey), StorageValue>,
        bytecodes: HashMap<B256, Bytecode>,
    }

    impl MockProvider {
        fn with_account(mut self, address: Address, account: Account) -> Self {
            self.accounts.insert(address, account);
            self
        }

        fn with_storage(mut self, address: Address, slot: StorageKey, value: StorageValue) -> Self {
            self.storage.insert((address, slot), value);
            self
        }
    }

    const fn idx(index: u64) -> BlockAccessIndex {
        BlockAccessIndex::new(index)
    }

    impl BlockHashReader for MockProvider {
        fn block_hash(
            &self,
            _number: alloy_primitives::BlockNumber,
        ) -> ProviderResult<Option<B256>> {
            Ok(None)
        }

        fn canonical_hashes_range(
            &self,
            _start: alloy_primitives::BlockNumber,
            _end: alloy_primitives::BlockNumber,
        ) -> ProviderResult<Vec<B256>> {
            Ok(Vec::new())
        }
    }

    impl AccountReader for MockProvider {
        fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
            Ok(self.accounts.get(address).copied())
        }
    }

    impl BytecodeReader for MockProvider {
        fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
            Ok(self.bytecodes.get(code_hash).cloned())
        }
    }

    impl StateRootProvider for MockProvider {
        fn state_root(&self, _hashed_state: HashedPostState) -> ProviderResult<B256> {
            Ok(B256::ZERO)
        }

        fn state_root_from_nodes(&self, _input: TrieInput) -> ProviderResult<B256> {
            Ok(B256::ZERO)
        }

        fn state_root_with_updates(
            &self,
            _hashed_state: HashedPostState,
        ) -> ProviderResult<(B256, TrieUpdates)> {
            Ok((B256::ZERO, TrieUpdates::default()))
        }

        fn state_root_from_nodes_with_updates(
            &self,
            _input: TrieInput,
        ) -> ProviderResult<(B256, TrieUpdates)> {
            Ok((B256::ZERO, TrieUpdates::default()))
        }
    }

    impl StorageRootProvider for MockProvider {
        fn storage_root(&self, _address: Address, _storage: HashedStorage) -> ProviderResult<B256> {
            Ok(B256::ZERO)
        }

        fn storage_proof(
            &self,
            _address: Address,
            _slot: B256,
            _hashed_storage: HashedStorage,
        ) -> ProviderResult<StorageProof> {
            unimplemented!()
        }

        fn storage_multiproof(
            &self,
            _address: Address,
            _slots: &[B256],
            _hashed_storage: HashedStorage,
        ) -> ProviderResult<StorageMultiProof> {
            unimplemented!()
        }
    }

    impl StateProofProvider for MockProvider {
        fn proof(
            &self,
            _input: TrieInput,
            _address: Address,
            _slots: &[B256],
        ) -> ProviderResult<AccountProof> {
            unimplemented!()
        }

        fn multiproof(
            &self,
            _input: TrieInput,
            _targets: MultiProofTargets,
        ) -> ProviderResult<MultiProof> {
            unimplemented!()
        }

        fn witness(
            &self,
            _input: TrieInput,
            _target: HashedPostState,
            _mode: ExecutionWitnessMode,
        ) -> ProviderResult<Vec<Bytes>> {
            Ok(Vec::new())
        }
    }

    impl HashedPostStateProvider for MockProvider {
        fn hashed_post_state(&self, _bundle_state: &BundleState) -> HashedPostState {
            HashedPostState::default()
        }
    }

    impl StateProvider for MockProvider {
        fn storage(
            &self,
            account: Address,
            storage_key: StorageKey,
        ) -> ProviderResult<Option<StorageValue>> {
            Ok(self.storage.get(&(account, storage_key)).copied())
        }
    }

    #[test]
    fn bal_overlay_reads_final_writes_before_parent_and_falls_back() {
        let address = Address::from([0x11; 20]);
        let untouched = Address::from([0x22; 20]);
        let slot = StorageKey::from(U256::from(1));
        let untouched_slot = StorageKey::from(U256::from(2));
        let parent_account = Account { balance: U256::from(1), nonce: 1, bytecode_hash: None };
        let fallback_account = Account { balance: U256::from(9), nonce: 9, bytecode_hash: None };
        let parent = MockProvider::default()
            .with_account(address, parent_account)
            .with_account(untouched, fallback_account)
            .with_storage(address, slot, U256::from(10))
            .with_storage(address, untouched_slot, U256::from(20));

        let mut overlay = BalStateOverlay::default();
        overlay.set_balance(address, U256::from(2));
        overlay.set_storage(address, slot, U256::from(11));
        let provider = overlay.provider(parent);

        let account = provider.basic_account(&address).unwrap().unwrap();
        assert_eq!(account.balance, U256::from(2));
        assert_eq!(account.nonce, 1);
        assert_eq!(provider.storage(address, slot).unwrap(), Some(U256::from(11)));
        assert_eq!(provider.storage(address, untouched_slot).unwrap(), Some(U256::from(20)));
        assert_eq!(provider.basic_account(&untouched).unwrap(), Some(fallback_account));
    }

    #[test]
    fn bal_overlay_handles_code_creation_deletion_zero_and_repeated_writes() {
        let address = Address::from([0x33; 20]);
        let deleted = Address::from([0x44; 20]);
        let empty = Address::from([0x45; 20]);
        let slot = StorageKey::from(U256::from(1));
        let repeated_slot = StorageKey::from(U256::from(2));
        let parent = MockProvider::default()
            .with_account(
                deleted,
                Account { balance: U256::from(1), nonce: 7, bytecode_hash: None },
            )
            .with_storage(deleted, slot, U256::from(99));

        let mut overlay = BalStateOverlay::default();
        overlay.set_balance(address, U256::from(5));
        overlay.set_nonce(address, 3);
        let code_hash = overlay.set_code_bytes(address, Bytes::from_static(&[0x60, 0x01])).unwrap();
        overlay.set_storage(address, slot, U256::ZERO);
        overlay.set_storage(address, repeated_slot, U256::from(1));
        overlay.set_storage(address, repeated_slot, U256::from(2));
        overlay.delete_account(deleted);
        overlay.set_account(empty, Account::default());

        let provider = overlay.provider(parent);
        let created = provider.basic_account(&address).unwrap().unwrap();
        assert_eq!(created.balance, U256::from(5));
        assert_eq!(created.nonce, 3);
        assert_eq!(created.bytecode_hash, Some(code_hash));
        assert!(provider.bytecode_by_hash(&code_hash).unwrap().is_some());
        assert_eq!(provider.storage(address, slot).unwrap(), Some(U256::ZERO));
        assert_eq!(provider.storage(address, repeated_slot).unwrap(), Some(U256::from(2)));
        assert_eq!(provider.basic_account(&deleted).unwrap(), None);
        assert_eq!(provider.storage(deleted, slot).unwrap(), Some(U256::ZERO));
        assert_eq!(provider.basic_account(&empty).unwrap(), Some(Account::default()));

        let hashed_overlay = provider.hashed_overlay_state().unwrap();
        assert_eq!(hashed_overlay.accounts.len(), 3);
        assert_eq!(hashed_overlay.storages.len(), 2);
    }

    #[test]
    fn bal_overlay_ignores_reads_and_uses_last_writes_from_account_changes() {
        let address = Address::from([0x55; 20]);
        let changed_slot = U256::from(1);
        let read_slot = U256::from(2);
        let parent = MockProvider::default()
            .with_account(
                address,
                Account { balance: U256::from(1), nonce: 1, bytecode_hash: None },
            )
            .with_storage(address, StorageKey::from(changed_slot), U256::from(10))
            .with_storage(address, StorageKey::from(read_slot), U256::from(20));
        let account_changes = AccountChanges {
            address,
            storage_changes: vec![SlotChanges::new(
                changed_slot,
                vec![
                    StorageChange::new(idx(0), U256::from(11)),
                    StorageChange::new(idx(1), U256::from(12)),
                ],
            )],
            storage_reads: vec![changed_slot, read_slot],
            balance_changes: vec![
                BalanceChange::new(idx(0), U256::from(2)),
                BalanceChange::new(idx(1), U256::from(3)),
            ],
            nonce_changes: vec![NonceChange::new(idx(0), 2), NonceChange::new(idx(1), 4)],
            code_changes: vec![CodeChange::new(idx(0), Bytes::new())],
        };

        let overlay = BalStateOverlay::from_account_changes([&account_changes]).unwrap();
        let provider = overlay.provider(parent);
        let account = provider.basic_account(&address).unwrap().unwrap();

        assert_eq!(account.balance, U256::from(3));
        assert_eq!(account.nonce, 4);
        assert_eq!(account.bytecode_hash, None);
        assert_eq!(
            provider.storage(address, StorageKey::from(changed_slot)).unwrap(),
            Some(U256::from(12))
        );
        assert_eq!(
            provider.storage(address, StorageKey::from(read_slot)).unwrap(),
            Some(U256::from(20))
        );
    }
}
