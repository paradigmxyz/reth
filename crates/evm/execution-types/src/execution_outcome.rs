use crate::{
    evm2_block_reverts_from_state_source, evm2_block_state_accumulator_extend,
    evm2_state_source_hashed_post_state, BlockExecutionOutput, BlockExecutionResult,
    Evm2BlockReverts, Evm2BlockState, Evm2StorageReverts,
};
use alloc::{collections::BTreeMap, vec, vec::Vec};
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_eips::eip7685::Requests;
use alloy_primitives::{
    logs_bloom,
    map::{AddressMap, B256Map, HashMap},
    Address, BlockNumber, Bloom, Log, B256, U256,
};
use evm2::evm::{
    AccountChangeRef, AccountInfo, AccountInfoRef, BlockAccountDelta, BlockStateAccumulator,
    StateChangeSink, StorageChangeRef,
};
use reth_primitives_traits::{Account, Bytecode, Receipt, StorageEntry};

/// Type used to initialize execution state.
pub type ExecutionStateInit = AddressMap<(Option<Account>, Option<Account>, B256Map<(U256, U256)>)>;

/// Types used inside `RevertsInit` to initialize reverts.
pub type AccountRevertInit = (Option<Option<Account>>, Vec<StorageEntry>);

/// Type used to initialize reverts.
pub type RevertsInit = HashMap<BlockNumber, AddressMap<AccountRevertInit>>;

/// Represents a changed account
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangedAccount {
    /// The address of the account.
    pub address: Address,
    /// Account nonce.
    pub nonce: u64,
    /// Account balance.
    pub balance: U256,
}

impl ChangedAccount {
    /// Creates a new [`ChangedAccount`] with the given address and 0 balance and nonce.
    pub const fn empty(address: Address) -> Self {
        Self { address, nonce: 0, balance: U256::ZERO }
    }
}

/// Represents the outcome of block execution, including post-execution changes and reverts.
///
/// The `ExecutionOutcome` structure aggregates the state changes over an arbitrary number of
/// blocks, capturing the resulting state, receipts, and requests following the execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionOutcome<T = reth_ethereum_primitives::Receipt> {
    /// Aggregated evm2 state changes.
    state: Evm2BlockState,
    /// Per-block evm2 state changes when available.
    block_states: Vec<Evm2BlockState>,
    /// Per-block reverts.
    block_reverts: Vec<Evm2BlockReverts>,
    /// The collection of receipts.
    /// Outer vector stores receipts for each block sequentially.
    /// The inner vector stores receipts ordered by transaction number.
    pub receipts: Vec<Vec<T>>,
    /// First block of execution state.
    pub first_block: BlockNumber,
    /// The collection of EIP-7685 requests.
    /// Outer vector stores requests for each block sequentially.
    /// The inner vector stores requests ordered by transaction number.
    ///
    /// A transaction may have zero or more requests, so the length of the inner vector is not
    /// guaranteed to be the same as the number of transactions.
    pub requests: Vec<Requests>,
}

impl<T> Default for ExecutionOutcome<T> {
    fn default() -> Self {
        Self {
            state: Default::default(),
            block_states: Default::default(),
            block_reverts: Default::default(),
            receipts: Default::default(),
            first_block: Default::default(),
            requests: Default::default(),
        }
    }
}

impl<T> ExecutionOutcome<T> {
    fn aggregate_block_states(states: &[Evm2BlockState]) -> Evm2BlockState {
        let mut accumulator = BlockStateAccumulator::new();
        for state in states {
            evm2_block_state_accumulator_extend(&mut accumulator, state);
        }
        accumulator.freeze()
    }

    fn from_parts(
        state: Evm2BlockState,
        block_states: Vec<Evm2BlockState>,
        block_reverts: Vec<Evm2BlockReverts>,
        receipts: Vec<Vec<T>>,
        first_block: BlockNumber,
        requests: Vec<Requests>,
    ) -> Self {
        Self { state, block_states, block_reverts, receipts, first_block, requests }
    }

    /// Creates a new `ExecutionOutcome` from evm2 aggregate state and per-block reverts.
    pub fn from_state_and_reverts(
        state: Evm2BlockState,
        block_reverts: Vec<Evm2BlockReverts>,
        receipts: Vec<Vec<T>>,
        first_block: BlockNumber,
        requests: Vec<Requests>,
    ) -> Self {
        let block_states =
            (block_reverts.len() <= 1).then(|| vec![state.clone()]).unwrap_or_default();
        Self::from_parts(state, block_states, block_reverts, receipts, first_block, requests)
    }

    /// Creates an empty execution outcome beginning at `first_block`.
    pub fn new_empty(first_block: BlockNumber) -> Self {
        Self::from_parts(
            Default::default(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            first_block,
            Vec::new(),
        )
    }

    /// Creates a new `ExecutionOutcome` from initialization parameters.
    ///
    /// This constructor initializes a new `ExecutionOutcome` instance using detailed
    /// initialization parameters.
    pub fn new_init(
        state_init: ExecutionStateInit,
        revert_init: RevertsInit,
        contracts_init: impl IntoIterator<Item = (B256, Bytecode)>,
        receipts: Vec<Vec<T>>,
        first_block: BlockNumber,
        requests: Vec<Requests>,
    ) -> Self {
        // sort reverts by block number
        let mut reverts = revert_init.into_iter().collect::<Vec<_>>();
        reverts.sort_unstable_by_key(|a| a.0);

        let mut accumulator = BlockStateAccumulator::new();
        for (address, (original, current, storage)) in state_init {
            accumulator
                .account(AccountChangeRef {
                    address,
                    original: original.as_ref().map(account_info_ref_from_reth),
                    current: current.as_ref().map(account_info_ref_from_reth),
                })
                .expect("infallible");
            for (slot, (original, current)) in storage {
                accumulator
                    .storage(StorageChangeRef {
                        address,
                        key: U256::from_be_bytes(slot.0),
                        original,
                        current,
                    })
                    .expect("infallible");
            }
        }
        for (code_hash, bytecode) in contracts_init {
            accumulator
                .bytecode(code_hash, &evm2::bytecode::Bytecode::new_raw(bytecode.original_bytes()))
                .expect("infallible");
        }

        let block_reverts = reverts
            .into_iter()
            .map(|(_, reverts)| Evm2BlockReverts {
                accounts: reverts
                    .iter()
                    .filter_map(|(address, (original, _))| {
                        original.map(|account| (*address, account.map(account_to_info)))
                    })
                    .collect(),
                storage: reverts
                    .into_iter()
                    .filter_map(|(address, (_, storage))| {
                        let slots = storage
                            .into_iter()
                            .map(|entry| (U256::from_be_bytes(entry.key.0), entry.value))
                            .collect::<BTreeMap<_, _>>();
                        (!slots.is_empty()).then_some((
                            address,
                            Evm2StorageReverts { slots, ..Default::default() },
                        ))
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();

        Self::from_parts(
            accumulator.freeze(),
            Vec::new(),
            block_reverts,
            receipts,
            first_block,
            requests,
        )
    }

    /// Creates a new `ExecutionOutcome` from a single block execution result.
    pub fn single(block_number: u64, output: BlockExecutionOutput<T>) -> Self {
        let block_reverts = evm2_block_reverts_from_state_source(&output.state);
        let state = output.state;
        Self {
            state: state.clone(),
            block_states: vec![state],
            block_reverts: vec![block_reverts],
            receipts: vec![output.result.receipts],
            first_block: block_number,
            requests: vec![output.result.requests],
        }
    }

    /// Appends a single block execution output.
    pub fn push_block(&mut self, block_number: u64, output: BlockExecutionOutput<T>) {
        self.extend(Self::single(block_number, output));
    }

    /// Creates a new `ExecutionOutcome` from multiple [`BlockExecutionResult`]s.
    fn from_blocks(
        first_block: u64,
        state: Evm2BlockState,
        block_states: Vec<Evm2BlockState>,
        block_reverts: Vec<Evm2BlockReverts>,
        results: Vec<BlockExecutionResult<T>>,
    ) -> Self {
        let mut value = Self::from_parts(
            state,
            block_states,
            block_reverts,
            Vec::with_capacity(results.len()),
            first_block,
            Vec::with_capacity(results.len()),
        );
        for result in results {
            value.receipts.push(result.receipts);
            value.requests.push(result.requests);
        }
        value
    }

    /// Creates a new `ExecutionOutcome` from evm2 block states and execution results.
    pub fn from_block_states(
        first_block: u64,
        states: impl IntoIterator<Item = Evm2BlockState>,
        results: Vec<BlockExecutionResult<T>>,
    ) -> Self {
        let block_states = states.into_iter().collect::<Vec<_>>();
        let mut block_reverts =
            block_states.iter().map(evm2_block_reverts_from_state_source).collect::<Vec<_>>();
        Self::adjust_reverts_for_prior_wipes(&Evm2BlockState::default(), &mut block_reverts);
        let state = Self::aggregate_block_states(&block_states);
        Self::from_blocks(first_block, state, block_states, block_reverts, results)
    }

    /// Returns mutable per-block reverts.
    pub const fn block_reverts_mut(&mut self) -> &mut Vec<Evm2BlockReverts> {
        &mut self.block_reverts
    }

    /// Returns per-block reverts.
    pub const fn block_reverts(&self) -> &Vec<Evm2BlockReverts> {
        &self.block_reverts
    }

    /// Returns changed storage slot indices for all changed accounts.
    pub fn storage_change_keys(&self) -> impl Iterator<Item = (Address, U256)> + '_ {
        self.state.storage().map(|(key, _)| (key.address(), key.key()))
    }

    /// Returns current changed storage values for `address`.
    pub fn storage_changes_for(&self, address: Address) -> impl Iterator<Item = (U256, U256)> + '_ {
        self.state
            .storage()
            .filter(move |(key, _)| key.address() == address)
            .map(|(key, value)| (key.key(), value.current))
    }

    /// Returns bytecodes changed by this execution outcome.
    pub fn bytecodes(&self) -> impl Iterator<Item = (B256, Bytecode)> + '_ {
        self.state
            .code()
            .filter(|(hash, _)| **hash != alloy_consensus::constants::KECCAK_EMPTY)
            .map(|(hash, bytecode)| (*hash, Bytecode::new_raw(bytecode.original_bytes())))
    }

    /// Returns the number of changed accounts.
    pub fn changed_account_count(&self) -> usize {
        self.state.accounts().count()
    }

    /// Returns the current state changes as an evm2 frozen block state.
    pub fn evm2_block_state(&self) -> Evm2BlockState {
        self.state.clone()
    }

    /// Set first block.
    pub const fn set_first_block(&mut self, first_block: BlockNumber) {
        self.first_block = first_block;
    }

    /// Return iterator over all accounts
    pub fn accounts_iter(&self) -> impl Iterator<Item = (Address, Option<&AccountInfo>)> {
        self.state.accounts().map(|(address, account)| (address, account.current.as_ref()))
    }

    /// Get account if account is known.
    pub fn account(&self, address: &Address) -> Option<Option<Account>> {
        self.account_state(address)
            .map(|account| account.current.as_ref().map(account_info_to_reth))
    }

    /// Returns the state account change for the given account.
    pub fn account_state(&self, address: &Address) -> Option<&BlockAccountDelta> {
        self.state
            .accounts()
            .find_map(|(changed, account)| (changed == *address).then_some(account))
    }

    /// Get storage if value is known.
    ///
    /// This means that depending on status we can potentially return `U256::ZERO`.
    pub fn storage(&self, address: &Address, storage_key: U256) -> Option<U256> {
        self.state.storage().find_map(|(key, storage)| {
            (key.address() == *address && key.key() == storage_key).then_some(storage.current)
        })
    }

    /// Return bytecode if known.
    pub fn bytecode(&self, code_hash: &B256) -> Option<Bytecode> {
        self.state
            .code()
            .find(|(hash, _)| *hash == code_hash)
            .map(|(_, bytecode)| Bytecode::new_raw(bytecode.original_bytes()))
    }

    /// Returns [`HashedPostState`] for this execution outcome.
    /// Returns the hashed post-state represented by the aggregate evm2 state.
    pub fn hash_state_slow<KH: reth_trie_common::KeyHasher>(
        &self,
    ) -> reth_trie_common::HashedPostState {
        evm2_state_source_hashed_post_state::<KH, _>(&self.state)
    }

    /// Transform block number to the index of block.
    pub const fn block_number_to_index(&self, block_number: BlockNumber) -> Option<usize> {
        if self.first_block > block_number {
            return None
        }
        let index = block_number - self.first_block;
        if index >= self.receipts.len() as u64 {
            return None
        }
        Some(index as usize)
    }

    /// Returns the receipt root for all recorded receipts.
    /// Note: this function calculated Bloom filters for every receipt and created merkle trees
    /// of receipt. This is an expensive operation.
    pub fn generic_receipts_root_slow(
        &self,
        block_number: BlockNumber,
        f: impl FnOnce(&[T]) -> B256,
    ) -> Option<B256> {
        Some(f(self.receipts.get(self.block_number_to_index(block_number)?)?))
    }

    /// Returns reference to receipts.
    pub const fn receipts(&self) -> &Vec<Vec<T>> {
        &self.receipts
    }

    /// Returns mutable reference to receipts.
    pub const fn receipts_mut(&mut self) -> &mut Vec<Vec<T>> {
        &mut self.receipts
    }

    /// Return all block receipts
    pub fn receipts_by_block(&self, block_number: BlockNumber) -> &[T] {
        let Some(index) = self.block_number_to_index(block_number) else { return &[] };
        &self.receipts[index]
    }

    /// Returns an iterator over receipt slices, one per block.
    ///
    /// This is a more ergonomic alternative to `receipts()` that yields slices
    /// instead of requiring indexing into a nested `Vec<Vec<T>>`.
    pub fn receipts_iter(&self) -> impl Iterator<Item = &[T]> + '_ {
        self.receipts.iter().map(|v| v.as_slice())
    }

    /// Is execution outcome empty.
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of blocks in the execution outcome.
    pub const fn len(&self) -> usize {
        self.receipts.len()
    }

    /// Return first block of the execution outcome
    pub const fn first_block(&self) -> BlockNumber {
        self.first_block
    }

    /// Return last block of the execution outcome
    pub const fn last_block(&self) -> BlockNumber {
        (self.first_block + self.len() as u64).saturating_sub(1)
    }

    /// Revert the state to the given block number.
    ///
    /// Returns false if the block number is not in the execution state.
    ///
    /// # Note
    ///
    /// The provided block number will stay inside the execution state.
    pub fn revert_to(&mut self, block_number: BlockNumber) -> bool {
        let Some(index) = self.block_number_to_index(block_number) else { return false };

        // +1 is for number of blocks that we have as index is included.
        let new_len = index + 1;
        // remove receipts
        self.receipts.truncate(new_len);
        // remove requests
        self.requests.truncate(new_len);
        // Revert last n reverts.
        self.block_reverts.truncate(new_len);
        self.truncate_block_states(new_len);

        true
    }

    /// Splits the block range state at a given block number.
    /// Returns two split states ([..at], [at..]).
    /// The plain state of the 2nd execution state will contain extra changes
    /// that were made in state transitions belonging to the lower state.
    ///
    /// # Panics
    ///
    /// If the target block number is not included in the state block range.
    pub fn split_at(self, at: BlockNumber) -> (Option<Self>, Self)
    where
        T: Clone,
    {
        if at == self.first_block {
            return (None, self)
        }

        let (mut lower_state, mut higher_state) = (self.clone(), self);

        // Revert lower state to [..at].
        lower_state.revert_to(at.checked_sub(1).unwrap());

        // Truncate higher state to [at..].
        let at_idx = higher_state.block_number_to_index(at).unwrap();
        higher_state.receipts = higher_state.receipts.split_off(at_idx);
        // Ensure that there are enough requests to truncate.
        // Sometimes we just have receipts and no requests.
        if at_idx < higher_state.requests.len() {
            higher_state.requests = higher_state.requests.split_off(at_idx);
        }
        higher_state.block_reverts.drain(..at_idx.min(higher_state.block_reverts.len()));
        higher_state.drop_first_block_states(at_idx);
        higher_state.first_block = at;

        (Some(lower_state), higher_state)
    }

    /// Extend one state from another
    ///
    /// For state this is very sensitive operation and should be used only when
    /// we know that other state was build on top of this one.
    /// In most cases this would be true.
    pub fn extend(&mut self, other: Self) {
        let Self {
            state: other_state,
            block_states: other_block_states,
            mut block_reverts,
            receipts,
            requests,
            ..
        } = other;
        let other_receipts_len = receipts.len();
        Self::adjust_reverts_for_prior_wipes(&self.state, &mut block_reverts);
        let mut accumulator = BlockStateAccumulator::new();
        evm2_block_state_accumulator_extend(&mut accumulator, &self.state);
        evm2_block_state_accumulator_extend(&mut accumulator, &other_state);
        self.state = accumulator.freeze();
        self.extend_block_states(other_block_states, other_receipts_len);
        self.block_reverts.extend(block_reverts);
        self.receipts.extend(receipts);
        self.requests.extend(requests);
    }

    fn truncate_block_states(&mut self, new_len: usize) {
        if !self.block_states.is_empty() {
            self.block_states.truncate(new_len);
            self.state = Self::aggregate_block_states(&self.block_states);
        }
    }

    fn drop_first_block_states(&mut self, n: usize) {
        if !self.block_states.is_empty() {
            self.block_states.drain(..n.min(self.block_states.len()));
            self.state = Self::aggregate_block_states(&self.block_states);
        }
    }

    fn extend_block_states(&mut self, other: Vec<Evm2BlockState>, other_receipts_len: usize) {
        if self.block_states.len() == self.receipts.len() && other.len() == other_receipts_len {
            self.block_states.extend(other);
        } else {
            self.block_states.clear();
        }
    }

    fn adjust_reverts_for_prior_wipes(
        prior_state: &Evm2BlockState,
        block_reverts: &mut [Evm2BlockReverts],
    ) {
        for address in prior_state.storage_wipes() {
            for reverts in block_reverts.iter_mut() {
                if let Some(storage_reverts) = reverts.storage.get_mut(&address) &&
                    storage_reverts.wiped
                {
                    storage_reverts.previous_wipe = true;
                    break
                }
            }
        }
    }

    /// Create a new instance with updated receipts.
    pub fn with_receipts(mut self, receipts: Vec<Vec<T>>) -> Self {
        self.receipts = receipts;
        self
    }

    /// Create a new instance with updated requests.
    pub fn with_requests(mut self, requests: Vec<Requests>) -> Self {
        self.requests = requests;
        self
    }

    /// Returns an iterator over all changed accounts from the `ExecutionOutcome`.
    ///
    /// This method filters the accounts to return only those that have undergone changes
    /// and maps them into `ChangedAccount` instances, which include the address, nonce, and
    /// balance.
    pub fn changed_accounts(&self) -> impl Iterator<Item = ChangedAccount> + '_ {
        self.accounts_iter().filter_map(|(addr, acc)| acc.map(|acc| (addr, acc))).map(
            |(address, acc)| ChangedAccount { address, nonce: acc.nonce, balance: acc.balance },
        )
    }
}

impl<T: Receipt<Log = Log>> ExecutionOutcome<T> {
    /// Returns an iterator over all block logs.
    pub fn logs(&self, block_number: BlockNumber) -> Option<impl Iterator<Item = &Log>> {
        let index = self.block_number_to_index(block_number)?;
        Some(self.receipts[index].iter().flat_map(|r| r.logs()))
    }

    /// Return blocks logs bloom
    pub fn block_logs_bloom(&self, block_number: BlockNumber) -> Option<Bloom> {
        Some(logs_bloom(self.logs(block_number)?))
    }
}

impl ExecutionOutcome {
    /// Returns the ethereum receipt root for all recorded receipts.
    ///
    /// Note: this function calculated Bloom filters for every receipt and created merkle trees
    /// of receipt. This is an expensive operation.
    pub fn ethereum_receipts_root(&self, block_number: BlockNumber) -> Option<B256> {
        self.generic_receipts_root_slow(
            block_number,
            reth_ethereum_primitives::calculate_receipt_root_no_memo,
        )
    }
}

impl<T> From<(BlockExecutionOutput<T>, BlockNumber)> for ExecutionOutcome<T> {
    fn from((output, block_number): (BlockExecutionOutput<T>, BlockNumber)) -> Self {
        Self::single(block_number, output)
    }
}

fn account_info_ref_from_reth(account: &Account) -> AccountInfoRef<'_> {
    AccountInfoRef {
        balance: account.balance,
        nonce: account.nonce,
        code_hash: account.get_bytecode_hash(),
        code: None,
    }
}

fn account_to_info(account: Account) -> AccountInfo {
    AccountInfo {
        balance: account.balance,
        nonce: account.nonce,
        code_hash: account.get_bytecode_hash(),
        code: None,
        _non_exhaustive: (),
    }
}

fn account_info_to_reth(info: &AccountInfo) -> Account {
    let bytecode_hash =
        (!info.code_hash.is_zero() && info.code_hash != KECCAK_EMPTY).then_some(info.code_hash);
    Account { nonce: info.nonce, balance: info.balance, bytecode_hash }
}

#[cfg(any(feature = "serde", feature = "serde-bincode-compat"))]
mod serde_state {
    use super::*;
    use alloy_primitives::Bytes;
    use evm2::{bytecode::Bytecode as Evm2Bytecode, evm::StateChangeSource};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub(super) struct BlockStateSerde {
        accounts: AddressMap<TrackedSerde<Option<AccountInfoSerde>>>,
        storage: AddressMap<StorageChangeSetSerde>,
        contracts: B256Map<Bytes>,
    }

    #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
    struct StorageChangeSetSerde {
        wipe: bool,
        slots: BTreeMap<U256, TrackedSerde<U256>>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct TrackedSerde<T> {
        original: T,
        current: T,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct AccountInfoSerde {
        balance: U256,
        nonce: u64,
        code_hash: B256,
        code: Option<Bytes>,
    }

    impl From<&Evm2BlockState> for BlockStateSerde {
        fn from(state: &Evm2BlockState) -> Self {
            let mut sink = BlockStateSerdeSink::default();
            match state.visit(&mut sink) {
                Ok(()) => {}
                Err(err) => match err {},
            }
            sink.state
        }
    }

    impl From<BlockStateSerde> for Evm2BlockState {
        fn from(value: BlockStateSerde) -> Self {
            let mut accumulator = BlockStateAccumulator::new();
            for (code_hash, bytecode) in value.contracts {
                accumulator
                    .bytecode(code_hash, &Evm2Bytecode::new_raw(bytecode))
                    .expect("infallible");
            }
            for (address, storage) in value.storage {
                if storage.wipe {
                    accumulator.storage_wipe(address).expect("infallible");
                }
                for (key, slot) in storage.slots {
                    accumulator
                        .storage(StorageChangeRef {
                            address,
                            key,
                            original: slot.original,
                            current: slot.current,
                        })
                        .expect("infallible");
                }
            }
            for (address, account) in value.accounts {
                let original = account.original.map(AccountInfo::from);
                let current = account.current.map(AccountInfo::from);
                accumulator
                    .account(AccountChangeRef {
                        address,
                        original: original.as_ref().map(account_info_ref),
                        current: current.as_ref().map(account_info_ref),
                    })
                    .expect("infallible");
            }
            accumulator.freeze()
        }
    }

    #[derive(Default)]
    struct BlockStateSerdeSink {
        state: BlockStateSerde,
    }

    impl Default for BlockStateSerde {
        fn default() -> Self {
            Self {
                accounts: AddressMap::default(),
                storage: AddressMap::default(),
                contracts: B256Map::default(),
            }
        }
    }

    impl StateChangeSink for BlockStateSerdeSink {
        type Error = core::convert::Infallible;

        fn bytecode(&mut self, code_hash: B256, code: &Evm2Bytecode) -> Result<(), Self::Error> {
            self.state.contracts.insert(code_hash, code.original_bytes());
            Ok(())
        }

        fn account(&mut self, change: AccountChangeRef<'_>) -> Result<(), Self::Error> {
            self.state.accounts.insert(
                change.address,
                TrackedSerde {
                    original: change.original.map(AccountInfoSerde::from),
                    current: change.current.map(AccountInfoSerde::from),
                },
            );
            Ok(())
        }

        fn storage_wipe(&mut self, address: Address) -> Result<(), Self::Error> {
            self.state.storage.entry(address).or_default().wipe = true;
            Ok(())
        }

        fn storage(&mut self, change: StorageChangeRef) -> Result<(), Self::Error> {
            self.state.storage.entry(change.address).or_default().slots.insert(
                change.key,
                TrackedSerde { original: change.original, current: change.current },
            );
            Ok(())
        }
    }

    fn account_info_ref(info: &AccountInfo) -> AccountInfoRef<'_> {
        AccountInfoRef {
            balance: info.balance,
            nonce: info.nonce,
            code_hash: info.code_hash,
            code: info.code.as_ref(),
        }
    }

    impl From<AccountInfoRef<'_>> for AccountInfoSerde {
        fn from(value: AccountInfoRef<'_>) -> Self {
            Self {
                balance: value.balance,
                nonce: value.nonce,
                code_hash: value.code_hash,
                code: value.code.map(Evm2Bytecode::original_bytes),
            }
        }
    }

    impl From<AccountInfoSerde> for AccountInfo {
        fn from(value: AccountInfoSerde) -> Self {
            Self {
                balance: value.balance,
                nonce: value.nonce,
                code_hash: value.code_hash,
                code: value.code.map(Evm2Bytecode::new_raw),
                _non_exhaustive: (),
            }
        }
    }
}

#[cfg(feature = "serde")]
mod serde_impl {
    use super::*;
    use alloc::vec::Vec;
    use alloy_eips::eip7685::Requests;
    use alloy_primitives::BlockNumber;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize)]
    struct ExecutionOutcomeSerde<'a, T> {
        state: serde_state::BlockStateSerde,
        block_reverts: &'a Vec<Evm2BlockReverts>,
        receipts: &'a Vec<Vec<T>>,
        first_block: BlockNumber,
        requests: &'a Vec<Requests>,
    }

    #[derive(Deserialize)]
    struct ExecutionOutcomeSerdeOwned<T> {
        state: serde_state::BlockStateSerde,
        block_reverts: Vec<Evm2BlockReverts>,
        receipts: Vec<Vec<T>>,
        first_block: BlockNumber,
        requests: Vec<Requests>,
    }

    impl<T> Serialize for ExecutionOutcome<T>
    where
        T: Serialize,
    {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            ExecutionOutcomeSerde {
                state: serde_state::BlockStateSerde::from(&self.state),
                block_reverts: &self.block_reverts,
                receipts: &self.receipts,
                first_block: self.first_block,
                requests: &self.requests,
            }
            .serialize(serializer)
        }
    }

    impl<'de, T> Deserialize<'de> for ExecutionOutcome<T>
    where
        T: Deserialize<'de>,
    {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            let value = ExecutionOutcomeSerdeOwned::<T>::deserialize(deserializer)?;
            Ok(ExecutionOutcome::from_state_and_reverts(
                value.state.into(),
                value.block_reverts,
                value.receipts,
                value.first_block,
                value.requests,
            ))
        }
    }
}

#[cfg(feature = "serde-bincode-compat")]
pub(super) mod serde_bincode_compat {
    use alloc::{borrow::Cow, vec::Vec};
    use alloy_eips::eip7685::Requests;
    use alloy_primitives::{BlockNumber, Bytes};
    use reth_primitives_traits::Receipt;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    /// Bincode-compatible [`super::ExecutionOutcome`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_execution_types::{serde_bincode_compat, ExecutionOutcome};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::ExecutionOutcome<'_>")]
    ///     chain: ExecutionOutcome,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub struct ExecutionOutcome<'a> {
        state: super::serde_state::BlockStateSerde,
        block_reverts: Vec<super::Evm2BlockReverts>,
        receipts: Vec<Vec<Bytes>>,
        first_block: BlockNumber,
        #[expect(clippy::owned_cow)]
        requests: Cow<'a, Vec<Requests>>,
    }

    impl<'a, T> From<&'a super::ExecutionOutcome<T>> for ExecutionOutcome<'a>
    where
        T: Receipt,
    {
        fn from(value: &'a super::ExecutionOutcome<T>) -> Self {
            ExecutionOutcome {
                state: super::serde_state::BlockStateSerde::from(&value.state),
                block_reverts: value.block_reverts.clone(),
                receipts: value
                    .receipts
                    .iter()
                    .map(|vec| {
                        vec.iter().map(|receipt| Bytes::from(alloy_rlp::encode(receipt))).collect()
                    })
                    .collect(),
                first_block: value.first_block,
                requests: Cow::Borrowed(&value.requests),
            }
        }
    }

    impl<T> From<ExecutionOutcome<'_>> for super::ExecutionOutcome<T>
    where
        T: Receipt,
    {
        fn from(value: ExecutionOutcome<'_>) -> Self {
            Self::from_state_and_reverts(
                value.state.into(),
                value.block_reverts,
                value
                    .receipts
                    .into_iter()
                    .map(|vec| {
                        vec.into_iter()
                            .map(|rlp| {
                                T::decode(&mut rlp.as_ref())
                                    .expect("invalid RLP for receipt in serde_bincode_compat")
                            })
                            .collect()
                    })
                    .collect(),
                value.first_block,
                value.requests.into_owned(),
            )
        }
    }

    impl<T> SerializeAs<super::ExecutionOutcome<T>> for ExecutionOutcome<'_>
    where
        T: Receipt,
    {
        fn serialize_as<S>(
            source: &super::ExecutionOutcome<T>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            ExecutionOutcome::from(source).serialize(serializer)
        }
    }

    impl<'de, T> DeserializeAs<'de, super::ExecutionOutcome<T>> for ExecutionOutcome<'de>
    where
        T: Receipt,
    {
        fn deserialize_as<D>(deserializer: D) -> Result<super::ExecutionOutcome<T>, D::Error>
        where
            D: Deserializer<'de>,
        {
            ExecutionOutcome::deserialize(deserializer).map(Into::into)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::super::{serde_bincode_compat, ExecutionOutcome};
        use rand::Rng;
        use reth_ethereum_primitives::Receipt;
        use serde::{Deserialize, Serialize};
        use serde_with::serde_as;

        #[test]
        fn test_chain_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data<T: reth_primitives_traits::Receipt> {
                #[serde_as(as = "serde_bincode_compat::ExecutionOutcome<'_>")]
                data: ExecutionOutcome<T>,
            }

            let mut bytes = [0u8; 1024];
            rand::rng().fill(bytes.as_mut_slice());
            let data = Data { data: ExecutionOutcome::new_empty(0) };

            let encoded = bincode::serialize(&data).unwrap();
            let decoded = bincode::deserialize::<Data<Receipt>>(&encoded).unwrap();
            assert_eq!(decoded, data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evm2_block_state_from_init;
    use alloy_consensus::TxType;
    use alloy_primitives::{bytes, Address, LogData};

    fn outcome_with_receipts<T>(
        first_block: BlockNumber,
        receipts: Vec<Vec<T>>,
        requests: Vec<Requests>,
    ) -> ExecutionOutcome<T> {
        ExecutionOutcome::new_empty(first_block).with_receipts(receipts).with_requests(requests)
    }

    #[test]
    fn test_initialization() {
        // Create a Receipts object with a vector of receipt vectors
        let receipts = vec![vec![Some(reth_ethereum_primitives::Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 46913,
            logs: vec![],
            success: true,
        })]];

        // Create a Requests object with a vector of requests
        let requests = vec![Requests::new(vec![bytes!("dead"), bytes!("beef"), bytes!("beebee")])];

        // Define the first block number
        let first_block = 123;

        // Create a ExecutionStateInit object and insert initial data
        let mut state_init: ExecutionStateInit = AddressMap::default();
        state_init
            .insert(Address::new([2; 20]), (None, Some(Account::default()), B256Map::default()));

        // Create an AddressMap for account reverts and insert initial data
        let mut revert_inner: AddressMap<AccountRevertInit> = AddressMap::default();
        revert_inner.insert(Address::new([2; 20]), (Some(None), vec![]));

        // Create a RevertsInit object and insert the revert_inner data
        let mut revert_init: RevertsInit = HashMap::default();
        revert_init.insert(first_block, revert_inner);

        let block_reverts = vec![Evm2BlockReverts {
            accounts: AddressMap::from_iter([(Address::new([2; 20]), None)]),
            storage: Default::default(),
        }];
        let state = evm2_block_state_from_init(
            state_init.clone().into_iter().map(|(address, (original, present, storage))| {
                (
                    address,
                    (original, present, storage.into_iter().map(|(k, s)| (k.into(), s)).collect()),
                )
            }),
            vec![],
        );

        let exec_res = ExecutionOutcome::from_state_and_reverts(
            state.clone(),
            block_reverts.clone(),
            receipts.clone(),
            first_block,
            requests.clone(),
        );

        // Assert that creating a new ExecutionOutcome using the constructor matches exec_res
        assert_eq!(
            ExecutionOutcome::from_state_and_reverts(
                state,
                block_reverts,
                receipts.clone(),
                first_block,
                requests.clone(),
            ),
            exec_res
        );

        // Assert that creating a new ExecutionOutcome using the new_init method matches
        // exec_res
        assert_eq!(
            ExecutionOutcome::new_init(
                state_init,
                revert_init,
                vec![],
                receipts,
                first_block,
                requests,
            ),
            exec_res
        );
    }

    #[test]
    fn test_block_number_to_index() {
        // Create a Receipts object with a vector of receipt vectors
        let receipts = vec![vec![Some(reth_ethereum_primitives::Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 46913,
            logs: vec![],
            success: true,
        })]];

        // Define the first block number
        let first_block = 123;

        // Create a ExecutionOutcome object with the created execution outcome, receipts, requests,
        // and first_block
        let exec_res = outcome_with_receipts(first_block, receipts, vec![]);

        // Test before the first block
        assert_eq!(exec_res.block_number_to_index(12), None);

        // Test after the first block but index larger than receipts length
        assert_eq!(exec_res.block_number_to_index(133), None);

        // Test after the first block
        assert_eq!(exec_res.block_number_to_index(123), Some(0));
    }

    #[test]
    fn test_get_logs() {
        // Create a Receipts object with a vector of receipt vectors
        let receipts = vec![vec![reth_ethereum_primitives::Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 46913,
            logs: vec![Log::<LogData>::default()],
            success: true,
        }]];

        // Define the first block number
        let first_block = 123;

        // Create a ExecutionOutcome object with the created execution outcome, receipts, requests,
        // and first_block
        let exec_res = outcome_with_receipts(first_block, receipts, vec![]);

        // Get logs for block number 123
        let logs: Vec<&Log> = exec_res.logs(123).unwrap().collect();

        // Assert that the logs match the expected logs
        assert_eq!(logs, vec![&Log::<LogData>::default()]);
    }

    #[test]
    fn test_receipts_by_block() {
        // Create a Receipts object with a vector of receipt vectors
        let receipts = vec![vec![Some(reth_ethereum_primitives::Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 46913,
            logs: vec![Log::<LogData>::default()],
            success: true,
        })]];

        // Define the first block number
        let first_block = 123;

        // Create a ExecutionOutcome object with the created execution outcome, receipts, requests,
        // and first_block
        let exec_res = outcome_with_receipts(first_block, receipts, vec![]);

        // Get receipts for block number 123 and convert the result into a vector
        let receipts_by_block: Vec<_> = exec_res.receipts_by_block(123).iter().collect();

        // Assert that the receipts for block number 123 match the expected receipts
        assert_eq!(
            receipts_by_block,
            vec![&Some(reth_ethereum_primitives::Receipt {
                tx_type: TxType::Legacy,
                cumulative_gas_used: 46913,
                logs: vec![Log::<LogData>::default()],
                success: true,
            })]
        );
    }

    #[test]
    fn test_receipts_len() {
        // Create a Receipts object with a vector of receipt vectors
        let receipts = vec![vec![Some(reth_ethereum_primitives::Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 46913,
            logs: vec![Log::<LogData>::default()],
            success: true,
        })]];

        // Create an empty Receipts object
        let receipts_empty = vec![];

        // Define the first block number
        let first_block = 123;

        // Create a ExecutionOutcome object with the created execution outcome, receipts, requests,
        // and first_block
        let exec_res = outcome_with_receipts(first_block, receipts, vec![]);

        // Assert that the length of receipts in exec_res is 1
        assert_eq!(exec_res.len(), 1);

        // Assert that exec_res is not empty
        assert!(!exec_res.is_empty());

        // Create a ExecutionOutcome object with an empty Receipts object
        let exec_res_empty_receipts: ExecutionOutcome =
            outcome_with_receipts(first_block, receipts_empty, vec![]);

        // Assert that the length of receipts in exec_res_empty_receipts is 0
        assert_eq!(exec_res_empty_receipts.len(), 0);

        // Assert that exec_res_empty_receipts is empty
        assert!(exec_res_empty_receipts.is_empty());
    }

    #[test]
    fn test_revert_to() {
        // Create a random receipt object
        let receipt = reth_ethereum_primitives::Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 46913,
            logs: vec![],
            success: true,
        };

        // Create a Receipts object with a vector of receipt vectors
        let receipts = vec![vec![Some(receipt.clone())], vec![Some(receipt.clone())]];

        // Define the first block number
        let first_block = 123;

        // Create a request.
        let request = bytes!("deadbeef");

        // Create a vector of Requests containing the request.
        let requests =
            vec![Requests::new(vec![request.clone()]), Requests::new(vec![request.clone()])];

        // Create a ExecutionOutcome object with the created execution outcome, receipts, requests,
        // and first_block
        let mut exec_res = outcome_with_receipts(first_block, receipts, requests);

        // Assert that the revert_to method returns true when reverting to the initial block number.
        assert!(exec_res.revert_to(123));

        // Assert that the receipts are properly cut after reverting to the initial block number.
        assert_eq!(exec_res.receipts, vec![vec![Some(receipt)]]);

        // Assert that the requests are properly cut after reverting to the initial block number.
        assert_eq!(exec_res.requests, vec![Requests::new(vec![request])]);

        // Assert that the revert_to method returns false when attempting to revert to a block
        // number greater than the initial block number.
        assert!(!exec_res.revert_to(133));

        // Assert that the revert_to method returns false when attempting to revert to a block
        // number less than the initial block number.
        assert!(!exec_res.revert_to(10));
    }

    #[test]
    fn test_extend_execution_outcome() {
        // Create a Receipt object with specific attributes.
        let receipt = reth_ethereum_primitives::Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 46913,
            logs: vec![],
            success: true,
        };

        // Create a Receipts object containing the receipt.
        let receipts = vec![vec![Some(receipt.clone())]];

        // Create a request.
        let request = bytes!("deadbeef");

        // Create a vector of Requests containing the request.
        let requests = vec![Requests::new(vec![request.clone()])];

        // Define the initial block number.
        let first_block = 123;

        // Create an ExecutionOutcome object.
        let mut exec_res = outcome_with_receipts(first_block, receipts, requests);

        // Extend the ExecutionOutcome object by itself.
        exec_res.extend(exec_res.clone());

        // Assert the extended ExecutionOutcome matches the expected outcome.
        assert_eq!(
            exec_res,
            outcome_with_receipts(
                123,
                vec![vec![Some(receipt.clone())], vec![Some(receipt)]],
                vec![Requests::new(vec![request.clone()]), Requests::new(vec![request])],
            )
        );
    }

    #[test]
    fn test_split_at_execution_outcome() {
        // Create a random receipt object
        let receipt = reth_ethereum_primitives::Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 46913,
            logs: vec![],
            success: true,
        };

        // Create a Receipts object with a vector of receipt vectors
        let receipts = vec![
            vec![Some(receipt.clone())],
            vec![Some(receipt.clone())],
            vec![Some(receipt.clone())],
        ];

        // Define the first block number
        let first_block = 123;

        // Create a request.
        let request = bytes!("deadbeef");

        // Create a vector of Requests containing the request.
        let requests = vec![
            Requests::new(vec![request.clone()]),
            Requests::new(vec![request.clone()]),
            Requests::new(vec![request.clone()]),
        ];

        // Create a ExecutionOutcome object with the created execution outcome, receipts, requests,
        // and first_block
        let exec_res = outcome_with_receipts(first_block, receipts, requests);

        // Split the ExecutionOutcome at block number 124
        let result = exec_res.clone().split_at(124);

        // Define the expected lower ExecutionOutcome after splitting
        let lower_execution_outcome = outcome_with_receipts(
            first_block,
            vec![vec![Some(receipt.clone())]],
            vec![Requests::new(vec![request.clone()])],
        );

        // Define the expected higher ExecutionOutcome after splitting
        let higher_execution_outcome = outcome_with_receipts(
            124,
            vec![vec![Some(receipt.clone())], vec![Some(receipt)]],
            vec![Requests::new(vec![request.clone()]), Requests::new(vec![request])],
        );

        // Assert that the split result matches the expected lower and higher outcomes
        assert_eq!(result.0, Some(lower_execution_outcome));
        assert_eq!(result.1, higher_execution_outcome);

        // Assert that splitting at the first block number returns None for the lower outcome
        assert_eq!(exec_res.clone().split_at(123), (None, exec_res));
    }

    #[test]
    fn test_changed_accounts() {
        // Set up some sample accounts
        let address1 = Address::random();
        let address2 = Address::random();
        let address3 = Address::random();

        let state = evm2_block_state_from_init(
            vec![
                (
                    address1,
                    (
                        None,
                        Some(Account { nonce: 1, balance: U256::from(100), bytecode_hash: None }),
                        BTreeMap::default(),
                    ),
                ),
                (
                    address2,
                    (
                        None,
                        Some(Account { nonce: 2, balance: U256::from(200), bytecode_hash: None }),
                        BTreeMap::default(),
                    ),
                ),
                (address3, (None, None, BTreeMap::default())),
            ],
            vec![],
        );

        let execution_outcome: ExecutionOutcome =
            ExecutionOutcome::from_state_and_reverts(state, vec![], Default::default(), 0, vec![]);

        // Get the changed accounts
        let changed_accounts: Vec<ChangedAccount> = execution_outcome.changed_accounts().collect();

        // Assert that the changed accounts match the expected ones
        assert_eq!(changed_accounts.len(), 2);

        assert!(changed_accounts.contains(&ChangedAccount {
            address: address1,
            nonce: 1,
            balance: U256::from(100)
        }));

        assert!(changed_accounts.contains(&ChangedAccount {
            address: address2,
            nonce: 2,
            balance: U256::from(200)
        }));
    }
}
