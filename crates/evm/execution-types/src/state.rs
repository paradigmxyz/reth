use alloc::{collections::BTreeMap, vec::Vec};
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{
    map::{AddressMap, AddressSet, B256Set, HashMap, HashSet},
    Address, B256, U256,
};
use core::{convert::Infallible, marker::PhantomData};
#[cfg(feature = "account-ext")]
use evm2::evm::AccountExtension;
use evm2::{
    bytecode::Bytecode as ExecutableBytecode,
    evm::{
        AccountChangeRef, AccountInfo, BlockStateAccumulator, StateChangeSink, StateChangeSource,
        StorageChange,
    },
};
use reth_primitives_traits::{Account, Bytecode as RethBytecode};
use reth_trie_common::{HashedPostState, KeyHasher};

/// Reverts for one block of execution state changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BlockReverts {
    /// Original accounts before the block changed them.
    pub accounts: AddressMap<Option<RevertAccount>>,
    /// Original storage before the block changed it.
    pub storage: AddressMap<StorageReverts>,
}

impl BlockReverts {
    /// Clears account and storage revert entries.
    pub fn clear(&mut self) {
        self.accounts.clear();
        self.storage.clear();
    }
}

/// Serializable account information used by block reverts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RevertAccount {
    /// Account balance.
    pub balance: U256,
    /// Account nonce.
    pub nonce: u64,
    /// Hash of the account bytecode, or the empty code hash.
    pub code_hash: B256,
    /// Optional account bytecode.
    pub code: Option<ExecutableBytecode>,
    /// Raw chain-specific account data before the block changed it.
    #[cfg(feature = "account-ext")]
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "AccountExtension::is_empty")
    )]
    pub extension: AccountExtension,
}

impl RevertAccount {
    /// Converts this account into account info.
    pub fn to_account_info(&self) -> AccountInfo {
        AccountInfo {
            balance: self.balance,
            nonce: self.nonce,
            code_hash: self.code_hash,
            code: self.code.clone(),
            #[cfg(feature = "account-ext")]
            extension: self.extension.clone(),
            _non_exhaustive: (),
        }
    }
}

impl From<AccountInfo> for RevertAccount {
    fn from(value: AccountInfo) -> Self {
        Self {
            balance: value.balance,
            nonce: value.nonce,
            code_hash: value.code_hash,
            code: value.code,
            #[cfg(feature = "account-ext")]
            extension: value.extension,
        }
    }
}

impl From<&AccountInfo> for RevertAccount {
    fn from(value: &AccountInfo) -> Self {
        Self {
            balance: value.balance,
            nonce: value.nonce,
            code_hash: value.code_hash,
            code: value.code.clone(),
            #[cfg(feature = "account-ext")]
            extension: value.extension.clone(),
        }
    }
}

impl From<RevertAccount> for AccountInfo {
    fn from(value: RevertAccount) -> Self {
        Self {
            balance: value.balance,
            nonce: value.nonce,
            code_hash: value.code_hash,
            code: value.code,
            #[cfg(feature = "account-ext")]
            extension: value.extension,
            _non_exhaustive: (),
        }
    }
}

impl From<&RevertAccount> for Account {
    fn from(value: &RevertAccount) -> Self {
        Self {
            nonce: value.nonce,
            balance: value.balance,
            bytecode_hash: (!value.code_hash.is_zero() && value.code_hash != KECCAK_EMPTY)
                .then_some(value.code_hash),
            #[cfg(feature = "account-ext")]
            extension: reth_primitives_traits::AccountExtension::from_shared(
                value.extension.clone().into_shared(),
            ),
        }
    }
}

/// Storage reverts for one account in one block.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StorageReverts {
    /// Whether the block wiped the account storage.
    pub wiped: bool,
    /// Whether earlier aggregate state had already marked this account storage as wiped.
    pub previous_wipe: bool,
    /// Original storage slots before the block changed them.
    pub slots: alloc::collections::BTreeMap<U256, RevertToSlot>,
}

/// Storage revert value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RevertToSlot {
    /// Revert the slot to the previous value observed by the transaction.
    Some(U256),
    /// The slot belonged to storage that was destroyed during the block.
    ///
    /// The transaction-local original value is not reliable in this case because the account may
    /// have been recreated at the same address and the slot may never have loaded its pre-block
    /// value. Storage writers resolve this marker from the pre-wipe database state.
    Destroyed,
}

impl Default for RevertToSlot {
    fn default() -> Self {
        Self::Some(U256::ZERO)
    }
}

impl RevertToSlot {
    /// Returns the previous value represented by this revert when no wiped database value is
    /// available.
    pub const fn to_previous_value(self) -> U256 {
        match self {
            Self::Some(value) => value,
            Self::Destroyed => U256::ZERO,
        }
    }
}

/// Returns addresses whose parent storage must be cleared before applying final slot values.
///
/// evm2 represents terminal deletion in account deltas and destruction followed by recreation
/// in storage wipes. Both need explicit parent-slot deletions at the trie boundary.
pub fn destroyed_accounts(state: &BlockStateAccumulator) -> impl Iterator<Item = Address> + '_ {
    state.storage_wipes().chain(state.accounts().filter_map(|(address, account)| {
        (account.original.is_some() && account.current.is_none()).then_some(address)
    }))
}

/// Returns the hashed updates represented by an execution state.
///
/// Providers must also materialize zero-valued parent slots for [`destroyed_accounts`].
pub fn hashed_post_state_from_execution_state<KH>(state: &BlockStateAccumulator) -> HashedPostState
where
    KH: KeyHasher,
{
    hashed_post_state_from_state_source::<KH, _>(state)
}

/// Returns the hashed post-state represented by an execution state-change source.
pub(crate) fn hashed_post_state_from_state_source<KH, S>(source: &S) -> HashedPostState
where
    KH: KeyHasher,
    S: StateChangeSource,
{
    let mut sink = HashedPostStateSink::<KH>::default();
    match source.visit(&mut sink) {
        Ok(()) => {}
        Err(err) => match err {},
    }
    sink.into_hashed_post_state()
}

/// Creates an execution state from Reth account, storage, and bytecode initialization data.
pub fn execution_state_from_init(
    accounts: impl IntoIterator<
        Item = (Address, (Option<Account>, Option<Account>, BTreeMap<U256, (U256, U256)>)),
    >,
    contracts: impl IntoIterator<Item = (B256, RethBytecode)>,
) -> BlockStateAccumulator {
    let mut accumulator = BlockStateAccumulator::new();
    for (address, (original, current, storage)) in accounts {
        let original = original.map(account_to_info);
        let current = current.map(account_to_info);
        accumulator
            .account(AccountChangeRef {
                address,
                original: original.as_ref(),
                current: current.as_ref(),
                created: false,
                selfdestructed: false,
            })
            .expect("infallible");
        for (slot, (original, current)) in storage {
            StateChangeSink::storage(
                &mut accumulator,
                StorageChange { address, key: slot, original, current },
            )
            .expect("infallible");
        }
    }
    for (code_hash, bytecode) in contracts {
        accumulator.bytecode(code_hash, &bytecode.into()).expect("infallible");
    }
    accumulator
}

/// Extends an execution state accumulator with any execution state-change source.
pub(crate) fn extend_execution_state<S>(accumulator: &mut BlockStateAccumulator, source: &S)
where
    S: StateChangeSource,
{
    match source.visit(accumulator) {
        Ok(()) => {}
        Err(err) => match err {},
    }
}

/// Rolls back a reconstructed aggregate whose per-block forward changes are unavailable.
pub(crate) fn revert_execution_state(
    state: &BlockStateAccumulator,
    reverts: &[BlockReverts],
    keep: usize,
) -> BlockStateAccumulator {
    let mut accounts = state
        .accounts()
        .map(|(address, delta)| (address, delta.clone()))
        .collect::<AddressMap<_>>();
    let mut wipes = state.storage_wipes().collect::<AddressSet>();
    let mut originals = state
        .storage()
        .map(|(key, delta)| ((key.address(), key.key()), delta.original))
        .collect::<HashMap<_, _>>();
    let mut storage = state
        .storage()
        .map(|(key, delta)| ((key.address(), key.key()), delta.current))
        .collect::<HashMap<_, _>>();

    // A net-zero change is absent from the aggregate, and a wipe discards earlier slot deltas.
    // The earliest revert supplies those original values for intermediate block states.
    let mut seen_slots: HashSet<(Address, U256)> = HashSet::default();
    for revert in reverts {
        for (address, account) in &revert.accounts {
            accounts.entry(*address).or_insert_with(|| {
                let original = account.as_ref().map(RevertAccount::to_account_info);
                evm2::evm::Tracked::from_parts(original.clone(), original)
            });
        }
        for (address, changes) in &revert.storage {
            for (key, value) in &changes.slots {
                if seen_slots.insert((*address, *key)) &&
                    let RevertToSlot::Some(value) = value
                {
                    if wipes.contains(address) {
                        originals.insert((*address, *key), *value);
                    } else {
                        originals.entry((*address, *key)).or_insert(*value);
                    }
                }
            }
        }
    }

    for revert in reverts.iter().skip(keep).rev() {
        for (address, changes) in &revert.storage {
            if changes.wiped {
                storage.retain(|(owner, _), _| owner != address);
                if changes.previous_wipe {
                    wipes.insert(*address);
                } else {
                    wipes.remove(address);
                }
            }
            for (key, value) in &changes.slots {
                match value {
                    RevertToSlot::Some(value) => {
                        storage.insert((*address, *key), *value);
                    }
                    RevertToSlot::Destroyed => {
                        storage.remove(&(*address, *key));
                    }
                }
            }
        }
        for (address, account) in &revert.accounts {
            accounts.get_mut(address).expect("revert accounts were collected").current =
                account.as_ref().map(RevertAccount::to_account_info);
        }
    }

    let mut reverted = BlockStateAccumulator::new();
    for (address, delta) in &accounts {
        let Ok(()) = reverted.account(AccountChangeRef {
            address: *address,
            original: delta.original.as_ref(),
            current: delta.current.as_ref(),
            created: false,
            selfdestructed: false,
        });
    }
    for address in wipes {
        let Ok(()) = reverted.storage_wipe(address);
    }
    for ((address, key), current) in storage {
        if accounts.get(&address).is_some_and(|account| account.current.is_none()) {
            continue;
        }
        let original = originals.get(&(address, key)).copied().unwrap_or_default();
        let Ok(()) = StateChangeSink::storage(
            &mut reverted,
            StorageChange { address, key, original, current },
        );
    }
    for (hash, code) in state.code() {
        let Ok(()) = reverted.bytecode(*hash, code);
    }
    reverted
}

/// Returns an approximate state-change size for thresholding and metrics.
pub(crate) fn state_source_size_hint(source: &BlockStateAccumulator) -> usize {
    source.accounts().count() +
        source.storage_wipes().count() +
        source.storage().count() +
        source.code().count()
}

/// Returns the per-block reverts represented by an execution state-change source.
pub(crate) fn block_reverts_from_state_source<S>(source: &S) -> BlockReverts
where
    S: StateChangeSource,
{
    let mut sink = BlockRevertsSink::default();
    match source.visit(&mut sink) {
        Ok(()) => {}
        Err(err) => match err {},
    }
    sink.reverts
}

/// Extends an execution state accumulator and returns the per-block reverts in one source visit.
pub(crate) fn extend_state_and_collect_reverts<S>(
    accumulator: &mut BlockStateAccumulator,
    source: &S,
) -> BlockReverts
where
    S: StateChangeSource,
{
    let mut sink = StateAndRevertsSink {
        accumulator,
        reverts: BlockRevertsSink::default(),
        block_wipes: AddressSet::default(),
    };
    match source.visit(&mut sink) {
        Ok(()) => {}
        Err(err) => match err {},
    }
    sink.reverts.reverts
}

/// Makes evm2's implicit deletion wipe explicit for Reth state consumers.
pub(crate) fn normalize_deleted_account_storage_wipes(state: &mut BlockStateAccumulator) {
    let deleted_accounts = state
        .accounts()
        .filter_map(|(address, account)| account.current.is_none().then_some(address))
        .collect::<Vec<_>>();
    for address in deleted_accounts {
        match state.storage_wipe(address) {
            Ok(()) => {}
            Err(err) => match err {},
        }
    }
}

/// Execution state-change sink that builds trie-ready hashed post-state as changes stream in.
#[derive(Debug)]
pub struct HashedPostStateSink<KH> {
    state: HashedPostState,
    created_accounts: B256Set,
    last_address: Option<(Address, B256)>,
    _key_hasher: PhantomData<KH>,
}

#[derive(Default)]
struct BlockRevertsSink {
    reverts: BlockReverts,
}

// A plain Tee cannot capture aggregate slots before a wipe clears them or retain the wipe after
// evm2 folds the following account deletion into the accumulator.
struct StateAndRevertsSink<'a> {
    accumulator: &'a mut BlockStateAccumulator,
    reverts: BlockRevertsSink,
    block_wipes: AddressSet,
}

impl StateAndRevertsSink<'_> {
    fn record_storage_wipe(&mut self, address: Address) -> Result<(), Infallible> {
        if self.block_wipes.insert(address) {
            let previous_wipe = self.accumulator.storage_wipes().any(|item| item == address);
            let prior_slots = BlockStateAccumulator::storage(self.accumulator)
                .filter(|(key, _)| key.address() == address)
                .map(|(key, value)| (key.key(), value.current))
                .collect::<Vec<_>>();
            let revert = self.reverts.reverts.storage.entry(address).or_default();
            revert.wiped = true;
            revert.previous_wipe = previous_wipe;
            for (key, value) in prior_slots {
                revert.slots.entry(key).or_insert(RevertToSlot::Some(value));
            }
        }
        self.accumulator.storage_wipe(address)
    }
}

impl StateChangeSink for StateAndRevertsSink<'_> {
    type Error = Infallible;

    fn bytecode(&mut self, code_hash: B256, code: &ExecutableBytecode) -> Result<(), Self::Error> {
        self.accumulator.bytecode(code_hash, code)
    }

    fn account(&mut self, change: AccountChangeRef<'_>) -> Result<(), Self::Error> {
        let deletes_account = change.current.is_none();
        let deletes_pre_aggregate_account = deletes_account &&
            self.accumulator
                .accounts()
                .find(|(address, _)| *address == change.address)
                .is_none_or(|(_, account)| account.original.is_some());
        self.reverts.account(change)?;
        if deletes_account && !self.block_wipes.contains(&change.address) {
            self.record_storage_wipe(change.address)?;
        }
        self.accumulator.account(change)?;
        if deletes_pre_aggregate_account {
            self.accumulator.storage_wipe(change.address)?;
        }
        Ok(())
    }

    fn storage_wipe(&mut self, address: Address) -> Result<(), Self::Error> {
        self.record_storage_wipe(address)
    }

    fn storage(&mut self, change: StorageChange) -> Result<(), Self::Error> {
        self.reverts.storage(change)?;
        self.accumulator.storage(change)
    }
}

impl<KH> Default for HashedPostStateSink<KH> {
    fn default() -> Self {
        Self {
            state: HashedPostState::default(),
            created_accounts: B256Set::default(),
            last_address: None,
            _key_hasher: PhantomData,
        }
    }
}

impl StateChangeSink for BlockRevertsSink {
    type Error = Infallible;

    fn bytecode(
        &mut self,
        _code_hash: B256,
        _code: &ExecutableBytecode,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn account(&mut self, change: AccountChangeRef<'_>) -> Result<(), Self::Error> {
        if change.original.is_some() && change.current.is_none() {
            self.storage_wipe(change.address)?;
        }
        self.reverts
            .accounts
            .entry(change.address)
            .or_insert_with(|| change.original.map(RevertAccount::from));
        Ok(())
    }

    fn storage_wipe(&mut self, address: Address) -> Result<(), Self::Error> {
        self.reverts.storage.entry(address).or_default().wiped = true;
        Ok(())
    }

    fn storage(&mut self, change: StorageChange) -> Result<(), Self::Error> {
        let revert = self.reverts.storage.entry(change.address).or_default();
        let previous = if revert.wiped {
            RevertToSlot::Destroyed
        } else {
            RevertToSlot::Some(change.original)
        };
        revert.slots.entry(change.key).or_insert(previous);
        Ok(())
    }
}

impl<KH> HashedPostStateSink<KH> {
    /// Consumes the sink and returns the accumulated hashed post-state.
    pub fn into_hashed_post_state(self) -> HashedPostState {
        self.state
    }

    fn hash_address(&mut self, address: Address) -> B256
    where
        KH: KeyHasher,
    {
        if let Some((previous, hash)) = self.last_address &&
            previous == address
        {
            return hash;
        }
        let hash = KH::hash_key(address);
        self.last_address = Some((address, hash));
        hash
    }

    fn drop_created_account_storage_wipe(&mut self, hashed_address: B256) {
        let remove_storage = if let Some(storage) = self.state.storages.get_mut(&hashed_address) {
            storage.storage.is_empty()
        } else {
            false
        };
        if remove_storage {
            self.state.storages.remove(&hashed_address);
        }
    }
}

impl<KH> StateChangeSink for HashedPostStateSink<KH>
where
    KH: KeyHasher,
{
    type Error = Infallible;

    fn bytecode(
        &mut self,
        _code_hash: B256,
        _code: &ExecutableBytecode,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn account(&mut self, change: AccountChangeRef<'_>) -> Result<(), Self::Error> {
        let hashed_address = self.hash_address(change.address);
        let was_created = self.created_accounts.contains(&hashed_address);
        let was_deleted_existing =
            !was_created && self.state.accounts.get(&hashed_address).is_some_and(Option::is_none);

        match change.current {
            Some(account) => {
                self.state.accounts.insert(hashed_address, Some(account_info_ref_to_reth(account)));

                if was_created || (change.original.is_none() && !was_deleted_existing) {
                    self.created_accounts.insert(hashed_address);
                    self.drop_created_account_storage_wipe(hashed_address);
                } else {
                    self.created_accounts.remove(&hashed_address);
                }
            }
            None if was_created => {
                self.state.accounts.remove(&hashed_address);
                self.created_accounts.remove(&hashed_address);
                self.state.storages.remove(&hashed_address);
            }
            None => {
                self.state.accounts.insert(hashed_address, None);
                self.created_accounts.remove(&hashed_address);
                // Preserve explicit deletions for slots observed before destruction. Parent
                // storage not observed here is expanded by the state provider at persistence.
                if let Some(storage) = self.state.storages.get_mut(&hashed_address) {
                    storage.storage.values_mut().for_each(|value| *value = U256::ZERO);
                }
            }
        }

        Ok(())
    }

    fn storage_wipe(&mut self, address: alloy_primitives::Address) -> Result<(), Self::Error> {
        let hashed_address = self.hash_address(address);
        if self.created_accounts.contains(&hashed_address) {
            self.state.storages.remove(&hashed_address);
        } else {
            if let Some(storage) = self.state.storages.get_mut(&hashed_address) {
                storage.storage.values_mut().for_each(|value| *value = U256::ZERO);
            }
        }
        Ok(())
    }

    fn storage(&mut self, change: StorageChange) -> Result<(), Self::Error> {
        let hashed_address = self.hash_address(change.address);
        let storage = self.state.storages.entry(hashed_address).or_default();
        storage.storage.insert(KH::hash_key(B256::new(change.key.to_be_bytes())), change.current);
        Ok(())
    }
}
fn account_info_ref_to_reth(info: &AccountInfo) -> Account {
    info.into()
}

fn account_to_info(account: Account) -> AccountInfo {
    account.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, U256};
    use evm2::evm::{AccountChangeRef, StorageChange, Tee};
    use reth_trie_common::KeccakKeyHasher;

    #[test]
    #[cfg(feature = "account-ext")]
    fn extension_only_updates_survive_hashing_and_block_reverts() {
        let address = Address::repeat_byte(0x42);
        let original = AccountInfo {
            nonce: 1,
            extension: AccountExtension::copy_from_slice(&[1; 32]),
            ..Default::default()
        };
        let current = AccountInfo {
            extension: AccountExtension::copy_from_slice(&[2; 32]),
            ..original.clone()
        };
        let mut pending = evm2::evm::PendingState::default();
        pending.insert_account(address, Some(original.clone()), Some(current.clone()));
        let mut state = BlockStateAccumulator::new();
        let first_revert = extend_state_and_collect_reverts(&mut state, &pending);
        let hashed = hashed_post_state_from_execution_state::<KeccakKeyHasher>(&state);
        let account = hashed.accounts[&alloy_primitives::keccak256(address)].as_ref().unwrap();
        assert_eq!(account.extension.as_ptr(), current.extension.as_ptr());
        assert_eq!(first_revert.accounts[&address].as_ref().unwrap().extension, original.extension);

        pending.insert_account(address, Some(current.clone()), Some(original.clone()));
        let second_revert = extend_state_and_collect_reverts(&mut state, &pending);
        assert_eq!(state.accounts().count(), 0);
        let reverted = revert_execution_state(&state, &[first_revert, second_revert], 1);
        let (_, delta) = reverted.accounts().next().unwrap();
        assert_eq!(delta.original.as_ref(), Some(&original));
        assert_eq!(delta.current.as_ref(), Some(&current));
    }
    const fn account_change<'a>(
        address: Address,
        original: Option<&'a AccountInfo>,
        current: Option<&'a AccountInfo>,
    ) -> AccountChangeRef<'a> {
        AccountChangeRef { address, original, current, created: false, selfdestructed: false }
    }

    #[test]
    fn deleted_account_preserves_storage_wipe_when_state_is_extended() {
        let address = Address::repeat_byte(0x01);
        let original = AccountInfo { balance: U256::from(1), nonce: 1, ..Default::default() };
        let mut source = BlockStateAccumulator::new();
        source.account(account_change(address, Some(&original), None)).unwrap();
        let mut aggregate = BlockStateAccumulator::new();

        let reverts = extend_state_and_collect_reverts(&mut aggregate, &source);

        assert_eq!(aggregate.storage_wipes().collect::<Vec<_>>(), [address]);
        assert!(reverts.storage.get(&address).is_some_and(|storage| storage.wiped));
    }

    #[test]
    fn storage_written_after_wipe_is_marked_destroyed() {
        let address = Address::repeat_byte(0x04);
        let key = U256::from(1);
        let mut source = BlockStateAccumulator::new();
        source.storage_wipe(address).unwrap();
        StateChangeSink::storage(
            &mut source,
            StorageChange { address, key, original: U256::ZERO, current: U256::from(1) },
        )
        .unwrap();

        let mut aggregate = BlockStateAccumulator::new();
        let reverts = extend_state_and_collect_reverts(&mut aggregate, &source);

        assert_eq!(
            reverts.storage[&address].slots[&key],
            RevertToSlot::Destroyed,
            "a post-wipe write must not use the recreated account's zero as the block revert"
        );
    }

    #[test]
    fn account_created_and_deleted_across_blocks_does_not_retain_aggregate_wipe() {
        let address = Address::repeat_byte(0x03);
        let account = AccountInfo { balance: U256::from(1), nonce: 1, ..Default::default() };
        let mut creation = BlockStateAccumulator::new();
        creation.account(account_change(address, None, Some(&account))).unwrap();
        let mut deletion = BlockStateAccumulator::new();
        deletion.account(account_change(address, Some(&account), None)).unwrap();
        let mut aggregate = BlockStateAccumulator::new();

        extend_state_and_collect_reverts(&mut aggregate, &creation);
        let deletion_reverts = extend_state_and_collect_reverts(&mut aggregate, &deletion);

        assert!(aggregate.accounts().next().is_none());
        assert!(aggregate.storage_wipes().next().is_none());
        assert!(deletion_reverts.storage.get(&address).is_some_and(|storage| storage.wiped));
    }

    #[test]
    fn hashed_post_state_sink_zeroes_prior_slots_on_storage_wipe() {
        let address = Address::repeat_byte(0x03);
        let mut sink = HashedPostStateSink::<KeccakKeyHasher>::default();

        sink.storage(StorageChange {
            address,
            key: U256::from(1),
            original: U256::ZERO,
            current: U256::from(2),
        })
        .unwrap();
        sink.storage_wipe(address).unwrap();
        sink.storage(StorageChange {
            address,
            key: U256::from(3),
            original: U256::ZERO,
            current: U256::from(4),
        })
        .unwrap();

        let hashed_state = sink.into_hashed_post_state();
        let storage = hashed_state.storages.get(&KeccakKeyHasher::hash_key(address)).unwrap();

        assert_eq!(storage.storage.len(), 2);
        assert_eq!(
            storage.storage[&KeccakKeyHasher::hash_key(B256::from(U256::from(1)))],
            U256::ZERO
        );
        assert_eq!(
            storage.storage.get(&KeccakKeyHasher::hash_key(B256::new(U256::from(3).to_be_bytes()))),
            Some(&U256::from(4))
        );
    }

    #[test]
    fn streaming_hashed_post_state_zeroes_observed_slots_for_deleted_accounts() {
        let address = Address::repeat_byte(0x04);
        let original = AccountInfo { balance: U256::from(1), nonce: 1, ..Default::default() };

        let mut accumulator = BlockStateAccumulator::new();
        let mut sink = HashedPostStateSink::<KeccakKeyHasher>::default();
        {
            let mut tee = Tee::new(&mut accumulator, &mut sink);
            tee.storage_wipe(address).unwrap();
            StateChangeSink::storage(
                &mut tee,
                StorageChange {
                    address,
                    key: U256::from(1),
                    original: U256::from(2),
                    current: U256::ZERO,
                },
            )
            .unwrap();
            tee.account(account_change(address, Some(&original), None)).unwrap();
        }

        let streaming = sink.into_hashed_post_state();
        let hashed_address = KeccakKeyHasher::hash_key(address);
        assert_eq!(streaming.accounts[&hashed_address], None);
        assert_eq!(
            streaming.storages[&hashed_address].storage
                [&KeccakKeyHasher::hash_key(B256::from(U256::from(1)))],
            U256::ZERO
        );
        let reverts = block_reverts_from_state_source(&accumulator);
        assert!(reverts.storage[&address].wiped);
    }

    #[test]
    fn streaming_hashed_post_state_drops_wipe_for_created_accounts() {
        let address = Address::repeat_byte(0x05);
        let current = AccountInfo { balance: U256::from(1), nonce: 1, ..Default::default() };

        let mut accumulator = BlockStateAccumulator::new();
        let mut sink = HashedPostStateSink::<KeccakKeyHasher>::default();
        {
            let mut tee = Tee::new(&mut accumulator, &mut sink);
            tee.storage_wipe(address).unwrap();
            StateChangeSink::storage(
                &mut tee,
                StorageChange {
                    address,
                    key: U256::from(1),
                    original: U256::ZERO,
                    current: U256::from(2),
                },
            )
            .unwrap();
            tee.account(account_change(address, None, Some(&current))).unwrap();
        }

        let recomputed =
            hashed_post_state_from_state_source::<KeccakKeyHasher, _>(&accumulator).into_sorted();
        let streaming = sink.into_hashed_post_state().into_sorted();

        assert_eq!(streaming, recomputed);
    }

    #[test]
    fn streaming_hashed_post_state_keeps_created_account_storage_wipes_local() {
        let address = Address::repeat_byte(0x06);
        let current = AccountInfo { balance: U256::from(1), nonce: 1, ..Default::default() };

        let mut accumulator = BlockStateAccumulator::new();
        let mut sink = HashedPostStateSink::<KeccakKeyHasher>::default();
        {
            let mut tee = Tee::new(&mut accumulator, &mut sink);
            tee.account(account_change(address, None, Some(&current))).unwrap();
            StateChangeSink::storage(
                &mut tee,
                StorageChange {
                    address,
                    key: U256::from(1),
                    original: U256::ZERO,
                    current: U256::from(2),
                },
            )
            .unwrap();
            tee.storage_wipe(address).unwrap();
        }

        let recomputed =
            hashed_post_state_from_state_source::<KeccakKeyHasher, _>(&accumulator).into_sorted();
        let streaming = sink.into_hashed_post_state().into_sorted();

        assert_eq!(streaming, recomputed);
    }

    #[test]
    fn streaming_hashed_post_state_keeps_created_marker_after_account_update() {
        let address = Address::repeat_byte(0x07);
        let current = AccountInfo { balance: U256::from(1), nonce: 1, ..Default::default() };
        let updated = AccountInfo { balance: U256::from(2), nonce: 1, ..Default::default() };

        let mut accumulator = BlockStateAccumulator::new();
        let mut sink = HashedPostStateSink::<KeccakKeyHasher>::default();
        {
            let mut tee = Tee::new(&mut accumulator, &mut sink);
            tee.account(account_change(address, None, Some(&current))).unwrap();
            tee.account(account_change(address, Some(&current), Some(&updated))).unwrap();
            tee.storage_wipe(address).unwrap();
        }

        let recomputed =
            hashed_post_state_from_state_source::<KeccakKeyHasher, _>(&accumulator).into_sorted();
        let streaming = sink.into_hashed_post_state().into_sorted();

        assert_eq!(streaming, recomputed);
    }

    #[test]
    fn streaming_hashed_post_state_keeps_wipe_for_existing_account_recreation() {
        let address = Address::repeat_byte(0x08);
        let original = AccountInfo { balance: U256::from(1), nonce: 1, ..Default::default() };
        let current = AccountInfo { balance: U256::from(2), nonce: 1, ..Default::default() };

        let mut accumulator = BlockStateAccumulator::new();
        let mut sink = HashedPostStateSink::<KeccakKeyHasher>::default();
        {
            let mut tee = Tee::new(&mut accumulator, &mut sink);
            tee.account(account_change(address, Some(&original), None)).unwrap();
            tee.storage_wipe(address).unwrap();
            StateChangeSink::storage(
                &mut tee,
                StorageChange {
                    address,
                    key: U256::from(1),
                    original: U256::ZERO,
                    current: U256::from(2),
                },
            )
            .unwrap();
            tee.account(account_change(address, None, Some(&current))).unwrap();
        }

        let recomputed =
            hashed_post_state_from_state_source::<KeccakKeyHasher, _>(&accumulator).into_sorted();
        let streaming = sink.into_hashed_post_state().into_sorted();

        assert_eq!(streaming, recomputed);
    }
}
