use alloc::{collections::BTreeMap, vec::Vec};
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{
    map::{AddressMap, AddressSet, B256Set},
    Address, B256, U256,
};
use core::{convert::Infallible, marker::PhantomData};
use evm2::{
    bytecode::Bytecode as ExecutableBytecode,
    evm::{
        AccountChangeRef, AccountInfo, AccountInfoRef, BlockStateAccumulator, StateChangeSink,
        StateChangeSource, StorageChange,
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
}

impl RevertAccount {
    /// Converts this account into account info.
    pub fn to_account_info(&self) -> AccountInfo {
        AccountInfo {
            balance: self.balance,
            nonce: self.nonce,
            code_hash: self.code_hash,
            code: self.code.clone(),
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
        }
    }
}

impl From<AccountInfoRef<'_>> for RevertAccount {
    fn from(value: AccountInfoRef<'_>) -> Self {
        Self {
            balance: value.balance,
            nonce: value.nonce,
            code_hash: value.code_hash,
            code: value.code.cloned(),
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
    pub slots: alloc::collections::BTreeMap<U256, U256>,
}

/// Returns the hashed post-state represented by an execution state.
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
        accumulator
            .account(AccountChangeRef {
                address,
                original: original.as_ref().map(account_ref_to_info_ref),
                current: current.as_ref().map(account_ref_to_info_ref),
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

/// Returns an approximate state-change size for thresholding and metrics.
pub(crate) fn state_source_size_hint<S>(source: &S) -> usize
where
    S: StateChangeSource,
{
    let mut sink = StateSizeHintSink::default();
    match source.visit(&mut sink) {
        Ok(()) => {}
        Err(err) => match err {},
    }
    sink.size
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
    _key_hasher: PhantomData<KH>,
}

#[derive(Default)]
struct StateSizeHintSink {
    size: usize,
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
                revert.slots.entry(key).or_insert(value);
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
        let deletes_pre_aggregate_account = change.deleted() &&
            self.accumulator
                .accounts()
                .find(|(address, _)| *address == change.address)
                .is_none_or(|(_, account)| account.original.is_some());
        self.reverts.account(change)?;
        if change.deleted() && !self.block_wipes.contains(&change.address) {
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
            _key_hasher: PhantomData,
        }
    }
}

impl StateChangeSink for StateSizeHintSink {
    type Error = Infallible;

    fn bytecode(
        &mut self,
        _code_hash: B256,
        _code: &ExecutableBytecode,
    ) -> Result<(), Self::Error> {
        self.size += 1;
        Ok(())
    }

    fn account(&mut self, _change: AccountChangeRef<'_>) -> Result<(), Self::Error> {
        self.size += 1;
        Ok(())
    }

    fn storage_wipe(&mut self, _address: alloy_primitives::Address) -> Result<(), Self::Error> {
        self.size += 1;
        Ok(())
    }

    fn storage(&mut self, _change: StorageChange) -> Result<(), Self::Error> {
        self.size += 1;
        Ok(())
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
        self.reverts
            .storage
            .entry(change.address)
            .or_default()
            .slots
            .entry(change.key)
            .or_insert(change.original);
        Ok(())
    }
}

impl<KH> HashedPostStateSink<KH> {
    /// Consumes the sink and returns the accumulated hashed post-state.
    pub fn into_hashed_post_state(self) -> HashedPostState {
        self.state
    }

    /// Takes the accumulated hashed post-state while leaving lifecycle markers intact.
    pub fn take_hashed_post_state(&mut self) -> HashedPostState {
        core::mem::take(&mut self.state)
    }

    fn drop_created_account_storage_wipe(&mut self, hashed_address: B256) {
        let remove_storage = if let Some(storage) = self.state.storages.get_mut(&hashed_address) {
            storage.wiped = false;
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
        let hashed_address = KH::hash_key(change.address);
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
                // The account leaves the trie, but persisted storage tables still need the wipe.
                let storage = self.state.storages.entry(hashed_address).or_default();
                storage.wiped = true;
                storage.storage.clear();
            }
        }

        Ok(())
    }

    fn storage_wipe(&mut self, address: alloy_primitives::Address) -> Result<(), Self::Error> {
        let hashed_address = KH::hash_key(address);
        if self.created_accounts.contains(&hashed_address) {
            self.state.storages.remove(&hashed_address);
        } else {
            let storage = self.state.storages.entry(hashed_address).or_default();
            storage.wiped = true;
            storage.storage.clear();
        }
        Ok(())
    }

    fn storage(&mut self, change: StorageChange) -> Result<(), Self::Error> {
        let hashed_address = KH::hash_key(change.address);
        let storage = self.state.storages.entry(hashed_address).or_default();
        if storage.wiped && change.current.is_zero() {
            return Ok(())
        }
        storage.storage.insert(KH::hash_key(B256::new(change.key.to_be_bytes())), change.current);
        Ok(())
    }
}
fn account_info_ref_to_reth(info: AccountInfoRef<'_>) -> Account {
    account_parts_to_reth(info.nonce, info.balance, info.code_hash)
}

fn account_ref_to_info_ref(account: &Account) -> AccountInfoRef<'_> {
    AccountInfoRef {
        balance: account.balance,
        nonce: account.nonce,
        code_hash: account.get_bytecode_hash(),
        code: None,
    }
}

fn account_parts_to_reth(nonce: u64, balance: alloy_primitives::U256, code_hash: B256) -> Account {
    let bytecode_hash = (!code_hash.is_zero() && code_hash != KECCAK_EMPTY).then_some(code_hash);
    Account { nonce, balance, bytecode_hash }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, U256};
    use evm2::evm::{AccountChangeRef, AccountInfoRef, StorageChange, Tee};
    use reth_trie_common::KeccakKeyHasher;

    #[test]
    fn revert_account_normalizes_empty_code_hashes() {
        for code_hash in [B256::ZERO, KECCAK_EMPTY] {
            let account = RevertAccount { code_hash, ..Default::default() };
            assert_eq!(Account::from(&account).bytecode_hash, None);
        }

        let code_hash = B256::repeat_byte(0x42);
        let account = RevertAccount { code_hash, ..Default::default() };
        assert_eq!(Account::from(&account).bytecode_hash, Some(code_hash));
    }

    #[test]
    fn deleted_account_preserves_storage_wipe_when_state_is_extended() {
        let address = Address::repeat_byte(0x01);
        let original =
            AccountInfoRef { balance: U256::from(1), nonce: 1, code_hash: B256::ZERO, code: None };
        let mut source = BlockStateAccumulator::new();
        source
            .account(AccountChangeRef { address, original: Some(original), current: None })
            .unwrap();
        let mut aggregate = BlockStateAccumulator::new();

        let reverts = extend_state_and_collect_reverts(&mut aggregate, &source);

        assert_eq!(aggregate.storage_wipes().collect::<Vec<_>>(), [address]);
        assert!(reverts.storage.get(&address).is_some_and(|storage| storage.wiped));
    }

    #[test]
    fn account_created_and_deleted_in_source_does_not_emit_storage_wipe() {
        let address = Address::repeat_byte(0x02);
        let account =
            AccountInfoRef { balance: U256::from(1), nonce: 1, code_hash: B256::ZERO, code: None };
        let mut source = BlockStateAccumulator::new();
        source
            .account(AccountChangeRef { address, original: None, current: Some(account) })
            .unwrap();
        source
            .account(AccountChangeRef { address, original: Some(account), current: None })
            .unwrap();
        let mut aggregate = BlockStateAccumulator::new();

        let reverts = extend_state_and_collect_reverts(&mut aggregate, &source);

        assert!(aggregate.accounts().next().is_none());
        assert!(aggregate.storage_wipes().next().is_none());
        assert!(reverts.storage.is_empty());
    }

    #[test]
    fn account_created_and_deleted_across_blocks_does_not_retain_aggregate_wipe() {
        let address = Address::repeat_byte(0x03);
        let account =
            AccountInfoRef { balance: U256::from(1), nonce: 1, code_hash: B256::ZERO, code: None };
        let mut creation = BlockStateAccumulator::new();
        creation
            .account(AccountChangeRef { address, original: None, current: Some(account) })
            .unwrap();
        let mut deletion = BlockStateAccumulator::new();
        deletion
            .account(AccountChangeRef { address, original: Some(account), current: None })
            .unwrap();
        let mut aggregate = BlockStateAccumulator::new();

        extend_state_and_collect_reverts(&mut aggregate, &creation);
        let deletion_reverts = extend_state_and_collect_reverts(&mut aggregate, &deletion);

        assert!(aggregate.accounts().next().is_none());
        assert!(aggregate.storage_wipes().next().is_none());
        assert!(deletion_reverts.storage.get(&address).is_some_and(|storage| storage.wiped));
    }

    #[test]
    fn hashed_post_state_sink_clears_prior_slots_on_storage_wipe() {
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

        assert!(storage.wiped);
        assert_eq!(storage.storage.len(), 1);
        assert_eq!(
            storage.storage.get(&KeccakKeyHasher::hash_key(B256::new(U256::from(3).to_be_bytes()))),
            Some(&U256::from(4))
        );
    }

    #[test]
    fn streaming_hashed_post_state_removes_storage_for_deleted_accounts() {
        let address = Address::repeat_byte(0x04);
        let original =
            AccountInfoRef { balance: U256::from(1), nonce: 1, code_hash: B256::ZERO, code: None };

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
            tee.account(AccountChangeRef { address, original: Some(original), current: None })
                .unwrap();
        }

        let recomputed =
            hashed_post_state_from_state_source::<KeccakKeyHasher, _>(&accumulator).into_sorted();
        let streaming = sink.into_hashed_post_state().into_sorted();

        assert_eq!(streaming, recomputed);
    }

    #[test]
    fn streaming_hashed_post_state_drops_wipe_for_created_accounts() {
        let address = Address::repeat_byte(0x05);
        let current =
            AccountInfoRef { balance: U256::from(1), nonce: 1, code_hash: B256::ZERO, code: None };

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
            tee.account(AccountChangeRef { address, original: None, current: Some(current) })
                .unwrap();
        }

        let recomputed =
            hashed_post_state_from_state_source::<KeccakKeyHasher, _>(&accumulator).into_sorted();
        let streaming = sink.into_hashed_post_state().into_sorted();

        assert_eq!(streaming, recomputed);
    }

    #[test]
    fn streaming_hashed_post_state_keeps_created_account_storage_wipes_local() {
        let address = Address::repeat_byte(0x06);
        let current =
            AccountInfoRef { balance: U256::from(1), nonce: 1, code_hash: B256::ZERO, code: None };

        let mut accumulator = BlockStateAccumulator::new();
        let mut sink = HashedPostStateSink::<KeccakKeyHasher>::default();
        {
            let mut tee = Tee::new(&mut accumulator, &mut sink);
            tee.account(AccountChangeRef { address, original: None, current: Some(current) })
                .unwrap();
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
        let current =
            AccountInfoRef { balance: U256::from(1), nonce: 1, code_hash: B256::ZERO, code: None };
        let updated =
            AccountInfoRef { balance: U256::from(2), nonce: 1, code_hash: B256::ZERO, code: None };

        let mut accumulator = BlockStateAccumulator::new();
        let mut sink = HashedPostStateSink::<KeccakKeyHasher>::default();
        {
            let mut tee = Tee::new(&mut accumulator, &mut sink);
            tee.account(AccountChangeRef { address, original: None, current: Some(current) })
                .unwrap();
            tee.account(AccountChangeRef {
                address,
                original: Some(current),
                current: Some(updated),
            })
            .unwrap();
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
        let original =
            AccountInfoRef { balance: U256::from(1), nonce: 1, code_hash: B256::ZERO, code: None };
        let current =
            AccountInfoRef { balance: U256::from(2), nonce: 1, code_hash: B256::ZERO, code: None };

        let mut accumulator = BlockStateAccumulator::new();
        let mut sink = HashedPostStateSink::<KeccakKeyHasher>::default();
        {
            let mut tee = Tee::new(&mut accumulator, &mut sink);
            tee.account(AccountChangeRef { address, original: Some(original), current: None })
                .unwrap();
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
            tee.account(AccountChangeRef { address, original: None, current: Some(current) })
                .unwrap();
        }

        let recomputed =
            hashed_post_state_from_state_source::<KeccakKeyHasher, _>(&accumulator).into_sorted();
        let streaming = sink.into_hashed_post_state().into_sorted();

        assert_eq!(streaming, recomputed);
    }
}
