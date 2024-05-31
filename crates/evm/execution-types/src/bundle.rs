use reth_evm::execute::BatchBlockExecutionOutput;
use reth_primitives::{
    logs_bloom,
    revm::compat::{into_reth_acc, into_revm_acc},
    Account, Address, BlockNumber, Bloom, Bytecode, Log, Receipt, Receipts, StorageEntry, B256,
    U256,
};
use reth_trie::HashedPostState;
use revm::{
    db::{states::BundleState, BundleAccount},
    primitives::AccountInfo,
};
use std::collections::HashMap;

/// Bundle state of post execution changes and reverts.
///
/// Aggregates the changes over an arbitrary number of blocks.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct BundleStateWithReceipts {
    /// Bundle state with reverts.
    pub bundle: BundleState,
    /// The collection of receipts.
    /// Outer vector stores receipts for each block sequentially.
    /// The inner vector stores receipts ordered by transaction number.
    ///
    /// If receipt is None it means it is pruned.
    pub receipts: Receipts,
    /// First block of bundle state.
    pub first_block: BlockNumber,
}

// TODO(mattsse): unify the types, currently there's a cyclic dependency between
impl From<BatchBlockExecutionOutput> for BundleStateWithReceipts {
    fn from(value: BatchBlockExecutionOutput) -> Self {
        let BatchBlockExecutionOutput { bundle, receipts, requests: _, first_block } = value;
        Self { bundle, receipts, first_block }
    }
}

// TODO(mattsse): unify the types, currently there's a cyclic dependency between
impl From<BundleStateWithReceipts> for BatchBlockExecutionOutput {
    fn from(value: BundleStateWithReceipts) -> Self {
        let BundleStateWithReceipts { bundle, receipts, first_block } = value;
        // TODO(alexey): add requests
        Self { bundle, receipts, requests: Vec::default(), first_block }
    }
}

/// Type used to initialize revms bundle state.
pub type BundleStateInit =
    HashMap<Address, (Option<Account>, Option<Account>, HashMap<B256, (U256, U256)>)>;

/// Types used inside RevertsInit to initialize revms reverts.
pub type AccountRevertInit = (Option<Option<Account>>, Vec<StorageEntry>);

/// Type used to initialize revms reverts.
pub type RevertsInit = HashMap<BlockNumber, HashMap<Address, AccountRevertInit>>;

impl BundleStateWithReceipts {
    /// Create Bundle State.
    pub const fn new(bundle: BundleState, receipts: Receipts, first_block: BlockNumber) -> Self {
        Self { bundle, receipts, first_block }
    }

    /// Create new bundle state with receipts.
    pub fn new_init(
        state_init: BundleStateInit,
        revert_init: RevertsInit,
        contracts_init: Vec<(B256, Bytecode)>,
        receipts: Receipts,
        first_block: BlockNumber,
    ) -> Self {
        // sort reverts by block number
        let mut reverts = revert_init.into_iter().collect::<Vec<_>>();
        reverts.sort_unstable_by_key(|a| a.0);

        // initialize revm bundle
        let bundle = BundleState::new(
            state_init.into_iter().map(|(address, (original, present, storage))| {
                (
                    address,
                    original.map(into_revm_acc),
                    present.map(into_revm_acc),
                    storage.into_iter().map(|(k, s)| (k.into(), s)).collect(),
                )
            }),
            reverts.into_iter().map(|(_, reverts)| {
                // does not needs to be sorted, it is done when taking reverts.
                reverts.into_iter().map(|(address, (original, storage))| {
                    (
                        address,
                        original.map(|i| i.map(into_revm_acc)),
                        storage.into_iter().map(|entry| (entry.key.into(), entry.value)),
                    )
                })
            }),
            contracts_init.into_iter().map(|(code_hash, bytecode)| (code_hash, bytecode.0)),
        );

        Self { bundle, receipts, first_block }
    }

    /// Return revm bundle state.
    pub const fn state(&self) -> &BundleState {
        &self.bundle
    }

    /// Returns mutable revm bundle state.
    pub fn state_mut(&mut self) -> &mut BundleState {
        &mut self.bundle
    }

    /// Set first block.
    pub fn set_first_block(&mut self, first_block: BlockNumber) {
        self.first_block = first_block;
    }

    /// Return iterator over all accounts
    pub fn accounts_iter(&self) -> impl Iterator<Item = (Address, Option<&AccountInfo>)> {
        self.bundle.state().iter().map(|(a, acc)| (*a, acc.info.as_ref()))
    }

    /// Return iterator over all [BundleAccount]s in the bundle
    pub fn bundle_accounts_iter(&self) -> impl Iterator<Item = (Address, &BundleAccount)> {
        self.bundle.state().iter().map(|(a, acc)| (*a, acc))
    }

    /// Get account if account is known.
    pub fn account(&self, address: &Address) -> Option<Option<Account>> {
        self.bundle.account(address).map(|a| a.info.clone().map(into_reth_acc))
    }

    /// Get storage if value is known.
    ///
    /// This means that depending on status we can potentially return U256::ZERO.
    pub fn storage(&self, address: &Address, storage_key: U256) -> Option<U256> {
        self.bundle.account(address).and_then(|a| a.storage_slot(storage_key))
    }

    /// Return bytecode if known.
    pub fn bytecode(&self, code_hash: &B256) -> Option<Bytecode> {
        self.bundle.bytecode(code_hash).map(Bytecode)
    }

    /// Returns [HashedPostState] for this bundle state.
    /// See [HashedPostState::from_bundle_state] for more info.
    pub fn hash_state_slow(&self) -> HashedPostState {
        HashedPostState::from_bundle_state(&self.bundle.state)
    }

    /// Transform block number to the index of block.
    fn block_number_to_index(&self, block_number: BlockNumber) -> Option<usize> {
        if self.first_block > block_number {
            return None
        }
        let index = block_number - self.first_block;
        if index >= self.receipts.len() as u64 {
            return None
        }
        Some(index as usize)
    }

    /// Returns an iterator over all block logs.
    pub fn logs(&self, block_number: BlockNumber) -> Option<impl Iterator<Item = &Log>> {
        let index = self.block_number_to_index(block_number)?;
        Some(self.receipts[index].iter().filter_map(|r| Some(r.as_ref()?.logs.iter())).flatten())
    }

    /// Return blocks logs bloom
    pub fn block_logs_bloom(&self, block_number: BlockNumber) -> Option<Bloom> {
        Some(logs_bloom(self.logs(block_number)?))
    }

    /// Returns the receipt root for all recorded receipts.
    /// Note: this function calculated Bloom filters for every receipt and created merkle trees
    /// of receipt. This is a expensive operation.
    pub fn receipts_root_slow(&self, _block_number: BlockNumber) -> Option<B256> {
        #[cfg(feature = "optimism")]
        panic!("This should not be called in optimism mode. Use `optimism_receipts_root_slow` instead.");
        #[cfg(not(feature = "optimism"))]
        self.receipts.root_slow(self.block_number_to_index(_block_number)?)
    }

    /// Returns the receipt root for all recorded receipts.
    /// Note: this function calculated Bloom filters for every receipt and created merkle trees
    /// of receipt. This is a expensive operation.
    #[cfg(feature = "optimism")]
    pub fn optimism_receipts_root_slow(
        &self,
        block_number: BlockNumber,
        chain_spec: &reth_primitives::ChainSpec,
        timestamp: u64,
    ) -> Option<B256> {
        self.receipts.optimism_root_slow(
            self.block_number_to_index(block_number)?,
            chain_spec,
            timestamp,
        )
    }

    /// Returns reference to receipts.
    pub const fn receipts(&self) -> &Receipts {
        &self.receipts
    }

    /// Returns mutable reference to receipts.
    pub fn receipts_mut(&mut self) -> &mut Receipts {
        &mut self.receipts
    }

    /// Return all block receipts
    pub fn receipts_by_block(&self, block_number: BlockNumber) -> &[Option<Receipt>] {
        let Some(index) = self.block_number_to_index(block_number) else { return &[] };
        &self.receipts[index]
    }

    /// Is bundle state empty of blocks.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of blocks in bundle state.
    pub fn len(&self) -> usize {
        self.receipts.len()
    }

    /// Return first block of the bundle
    pub const fn first_block(&self) -> BlockNumber {
        self.first_block
    }

    /// Revert the state to the given block number.
    ///
    /// Returns false if the block number is not in the bundle state.
    ///
    /// # Note
    ///
    /// The provided block number will stay inside the bundle state.
    pub fn revert_to(&mut self, block_number: BlockNumber) -> bool {
        let Some(index) = self.block_number_to_index(block_number) else { return false };

        // +1 is for number of blocks that we have as index is included.
        let new_len = index + 1;
        let rm_trx: usize = self.len() - new_len;

        // remove receipts
        self.receipts.truncate(new_len);
        // Revert last n reverts.
        self.bundle.revert(rm_trx);

        true
    }

    /// Splits the block range state at a given block number.
    /// Returns two split states ([..at], [at..]).
    /// The plain state of the 2nd bundle state will contain extra changes
    /// that were made in state transitions belonging to the lower state.
    ///
    /// # Panics
    ///
    /// If the target block number is not included in the state block range.
    pub fn split_at(self, at: BlockNumber) -> (Option<Self>, Self) {
        if at == self.first_block {
            return (None, self)
        }

        let (mut lower_state, mut higher_state) = (self.clone(), self);

        // Revert lower state to [..at].
        lower_state.revert_to(at.checked_sub(1).unwrap());

        // Truncate higher state to [at..].
        let at_idx = higher_state.block_number_to_index(at).unwrap();
        higher_state.receipts = Receipts::from_vec(higher_state.receipts.split_off(at_idx));
        higher_state.bundle.take_n_reverts(at_idx);
        higher_state.first_block = at;

        (Some(lower_state), higher_state)
    }

    /// Extend one state from another
    ///
    /// For state this is very sensitive operation and should be used only when
    /// we know that other state was build on top of this one.
    /// In most cases this would be true.
    pub fn extend(&mut self, other: Self) {
        self.bundle.extend(other.bundle);
        self.receipts.extend(other.receipts.receipt_vec);
    }

    /// Prepends present the state with the given BundleState.
    /// It adds changes from the given state but does not override any existing changes.
    ///
    /// Reverts  and receipts are not updated.
    pub fn prepend_state(&mut self, mut other: BundleState) {
        let other_len = other.reverts.len();
        // take this bundle
        let this_bundle = std::mem::take(&mut self.bundle);
        // extend other bundle with this
        other.extend(this_bundle);
        // discard other reverts
        other.take_n_reverts(other_len);
        // swap bundles
        std::mem::swap(&mut self.bundle, &mut other)
    }
}
