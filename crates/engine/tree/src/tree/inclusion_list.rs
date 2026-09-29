//! EIP-7805 (FOCIL) inclusion-list handling for the engine tree.
//!
//! Covers the appendability check that decides whether a block satisfies the inclusion list it
//! was given, and the bounded store of lists retained from `engine_newPayloadV6`.

use alloy_consensus::{constants::KECCAK_EMPTY, Transaction};
use alloy_eips::{
    eip2718::Decodable2718,
    eip4844::{DATA_GAS_PER_BLOB, VERSIONED_HASH_VERSION_KZG},
};
use alloy_primitives::{
    map::{AddressMap, B256Map, B256Set},
    Bytes, B256, U256,
};
use reth_errors::ProviderResult;
use reth_primitives_traits::{BlockBody as _, NodePrimitives, RecoveredBlock, SignedTransaction};
use reth_provider::StateProviderBox;
use revm::{
    context_interface::cfg::gas_params::Eip2780TxInfo, interpreter::gas::calculate_initial_tx_gas,
    primitives::hardfork::SpecId,
};
use std::collections::VecDeque;

/// Block-scoped inputs for the EIP-7805 appendability check, taken from the payload's own EVM
/// environment so the check follows the fork the block was executed under.
#[derive(Debug, Clone, Copy)]
pub(super) struct InclusionListContext {
    pub(super) chain_id: u64,
    pub(super) spec_id: SpecId,
    pub(super) base_fee_per_gas: Option<u64>,
    /// Gas still unspent at the end of the block.
    pub(super) available_gas: u64,
    /// EIP-7825 cap on a single transaction's gas limit.
    pub(super) tx_gas_limit_cap: u64,
    /// EIP-3860 init code bound, as raised by EIP-7954 from Amsterdam on.
    pub(super) max_initcode_size: usize,
    /// Blob gas still unspent at the end of the block.
    pub(super) blob_gas_available: u64,
    /// The block's blob gas price, from its own excess blob gas.
    pub(super) blob_gas_price: u128,
    /// EIP-7840 cap on the blobs a single transaction may carry.
    pub(super) max_blobs_per_tx: Option<u64>,
}

/// Returns whether the block satisfies its EIP-7805 inclusion list, i.e. no inclusion-list
/// transaction missing from the block could have been validly appended to it.
///
/// Reports status only: an unsatisfied inclusion list does not make a payload invalid.
pub(super) fn inclusion_list_satisfied<N: NodePrimitives>(
    block: &RecoveredBlock<N::Block>,
    state: &StateProviderBox,
    ctx: &InclusionListContext,
    transactions: &[Bytes],
) -> ProviderResult<bool> {
    let included = block
        .body()
        .transactions_iter()
        .map(SignedTransaction::recalculate_hash)
        .collect::<B256Set>();
    let withdrawn = withdrawal_credits::<N>(block);

    for encoded in transactions {
        let Ok(transaction) = N::SignedTx::decode_2718_exact(encoded) else { continue };
        if included.contains(&transaction.recalculate_hash()) {
            continue
        }
        if could_append_transaction::<N>(&transaction, state, ctx, &withdrawn)? {
            return Ok(false)
        }
    }
    Ok(true)
}

/// Wei this block credited to each address by withdrawal.
///
/// The spec's `apply_body` runs the inclusion-list check after the block's transactions but before
/// `process_withdrawals`, so a sender funded only by a withdrawal in the same block is not yet
/// includable. The state read by the check is the block's post-state, which already holds those
/// credits; withdrawals only add balance and never touch nonce or code, so subtracting them per
/// address reconstructs the balance the spec sees.
fn withdrawal_credits<N: NodePrimitives>(block: &RecoveredBlock<N::Block>) -> AddressMap<U256> {
    let mut credits = AddressMap::default();
    for withdrawal in block.body().withdrawals().into_iter().flatten() {
        *credits.entry(withdrawal.address).or_insert(U256::ZERO) += withdrawal.amount_wei();
    }
    credits
}

/// Returns whether `transaction` could have been validly appended to the end of the block.
///
/// Mirrors the execution spec's `check_inclusion_list_transactions`
/// (`src/ethereum/forks/amsterdam/fork.py`), which admits a transaction that passes
/// `validate_transaction` and `check_transaction` against the block's post-transaction state.
///
/// Conditions rejected here must also be rejected by the payload builder, otherwise reth reports
/// its own blocks as unsatisfied. Blob transactions are a known exception: the spec judges them
/// appendable, while the payload builder skips every EIP-4844 transaction in an inclusion list
/// instead of looking its sidecar up in the blob store. The two can only disagree on a list
/// aggregated from another proposer, since `engine_getInclusionListV1` does not offer blob
/// transactions of our own.
fn could_append_transaction<N: NodePrimitives>(
    transaction: &N::SignedTx,
    state: &StateProviderBox,
    ctx: &InclusionListContext,
    withdrawn: &AddressMap<U256>,
) -> ProviderResult<bool> {
    // EIP-2681 reserves the maximum nonce; execution could not increment past it.
    if transaction.nonce() == u64::MAX {
        return Ok(false)
    }

    // EIP-4844 form, from the spec's `validate_transaction`: a blob transaction must carry blobs,
    // no more than the EIP-7840 per-transaction cap, and only KZG-versioned hashes.
    if let Some(blob_versioned_hashes) = transaction.blob_versioned_hashes() {
        if blob_versioned_hashes.is_empty() ||
            ctx.max_blobs_per_tx.is_some_and(|max| blob_versioned_hashes.len() as u64 > max) ||
            blob_versioned_hashes.iter().any(|hash| hash[0] != VERSIONED_HASH_VERSION_KZG)
        {
            return Ok(false)
        }

        // The blob dimension has its own remaining budget (`check_block_gas_capacity`), and the
        // fee cap is measured against the block's blob gas price (`check_max_fee_per_blob_gas`).
        if blob_gas(transaction) > ctx.blob_gas_available ||
            transaction.max_fee_per_blob_gas().unwrap_or_default() < ctx.blob_gas_price
        {
            return Ok(false)
        }
    }

    // EIP-7702 requires a non-empty authorization list.
    if transaction.authorization_list().is_some_and(|list| list.is_empty()) {
        return Ok(false)
    }

    // Block gas capacity. The EIP-7825 cap is not a bound on the gas limit itself; it bounds
    // regular gas only, and is checked against intrinsic gas below.
    if transaction.gas_limit() > ctx.available_gas {
        return Ok(false)
    }

    // A legacy transaction without a chain id is replay-protected by omission, so only a
    // mismatch disqualifies.
    if transaction.chain_id().is_some_and(|chain_id| chain_id != ctx.chain_id) {
        return Ok(false)
    }

    // EIP-3860 init code bound. The limit is fork-dependent — EIP-7954 raises it in Amsterdam —
    // so it is taken from the block's own EVM environment.
    if transaction.is_create() && transaction.input().len() > ctx.max_initcode_size {
        return Ok(false)
    }

    // Base fee coverage and the EIP-1559 fee-cap ordering rule.
    if ctx.base_fee_per_gas.is_some_and(|base_fee| transaction.max_fee_per_gas() < base_fee as u128) ||
        transaction
            .max_priority_fee_per_gas()
            .is_some_and(|tip| tip > transaction.max_fee_per_gas())
    {
        return Ok(false)
    }

    let Ok(sender) = transaction.try_recover() else { return Ok(false) };

    let intrinsic_gas = calculate_initial_tx_gas(
        ctx.spec_id,
        transaction.input(),
        transaction.is_create(),
        transaction.access_list().map_or(0, |list| list.len()) as u64,
        transaction
            .access_list()
            .map_or(0, |list| list.iter().map(|item| item.storage_keys.len()).sum()) as u64,
        transaction.authorization_list().map_or(0, |list| list.len()) as u64,
        Some(Eip2780TxInfo {
            value: transaction.value(),
            is_self_transfer: transaction.kind().to() == Some(&sender),
        }),
    );
    if transaction.gas_limit() < intrinsic_gas.initial_total_gas() ||
        transaction.gas_limit() < intrinsic_gas.floor_gas
    {
        return Ok(false)
    }

    // EIP-8037 caps regular gas, not the transaction's gas limit: a limit above the cap is legal
    // as long as the intrinsic regular gas fits, since state gas draws on its own reservoir.
    if ctx.spec_id >= SpecId::AMSTERDAM &&
        intrinsic_gas.initial_regular_gas().max(intrinsic_gas.floor_gas) > ctx.tx_gas_limit_cap
    {
        return Ok(false)
    }

    let account = state.basic_account(&sender)?.unwrap_or_default();
    // The balance the spec sees precedes this block's withdrawals; see `withdrawal_credits`.
    let balance =
        account.balance.saturating_sub(withdrawn.get(&sender).copied().unwrap_or(U256::ZERO));

    // An account carrying code is not an EOA unless the code is an EIP-7702 delegation, mirroring
    // the spec's `sender_account.code_hash != EMPTY_CODE_HASH`.
    //
    // The comparison is against the code hash rather than `Account::has_bytecode`, because a state
    // provider may report a codeless account as `Some(KECCAK_EMPTY)`: only accounts that went
    // through `Account::from_revm_account` are normalized to `None`. `has_bytecode` would classify
    // most plain senders as contracts.
    if account.get_bytecode_hash() != KECCAK_EMPTY &&
        !state.account_code(&sender)?.is_some_and(|code| code.is_eip7702())
    {
        return Ok(false)
    }

    let max_gas_cost = U256::from(transaction.gas_limit())
        .checked_mul(U256::from(transaction.max_fee_per_gas()))
        .unwrap_or(U256::MAX);
    // A blob transaction also prepays its blob gas at its own fee cap.
    let max_blob_cost = U256::from(blob_gas(transaction))
        .checked_mul(U256::from(transaction.max_fee_per_blob_gas().unwrap_or_default()))
        .unwrap_or(U256::MAX);
    let max_cost = max_gas_cost
        .checked_add(max_blob_cost)
        .and_then(|cost| cost.checked_add(transaction.value()))
        .unwrap_or(U256::MAX);

    Ok(account.nonce == transaction.nonce() && balance >= max_cost)
}

/// Blob gas a transaction consumes, zero for every non-blob type.
fn blob_gas(transaction: &impl Transaction) -> u64 {
    transaction.blob_versioned_hashes().map_or(0, |hashes| hashes.len() as u64) * DATA_GAS_PER_BLOB
}

/// Upper bound on the inclusion lists retained from `engine_newPayloadV6`.
const MAX_RETAINED_INCLUSION_LISTS: usize = 64;

/// Inclusion lists retained from `engine_newPayloadV6`, keyed by block hash, with the cached
/// satisfaction verdict for each.
///
/// EIP-7805 permits discarding a list once its payload is no longer a branch tip, so a bounded
/// FIFO window suffices: an evicted entry only leaves `inclusionListSatisfied` unreported.
#[derive(Debug, Default)]
pub(super) struct RetainedInclusionLists {
    lists: B256Map<Vec<Bytes>>,
    results: B256Map<bool>,
    order: VecDeque<B256>,
}

impl RetainedInclusionLists {
    /// Retains `transactions` for `block_hash` and invalidates any cached verdict for it.
    pub(super) fn insert(&mut self, block_hash: B256, transactions: Vec<Bytes>) {
        self.results.remove(&block_hash);
        if self.lists.insert(block_hash, transactions).is_none() {
            self.order.push_back(block_hash);
        }
        while self.order.len() > MAX_RETAINED_INCLUSION_LISTS {
            if let Some(evicted) = self.order.pop_front() {
                self.lists.remove(&evicted);
                self.results.remove(&evicted);
            }
        }
    }

    pub(super) fn get(&self, block_hash: &B256) -> Option<&Vec<Bytes>> {
        self.lists.get(block_hash)
    }

    pub(super) fn cached_result(&self, block_hash: &B256) -> Option<bool> {
        self.results.get(block_hash).copied()
    }

    pub(super) fn cache_result(&mut self, block_hash: B256, satisfied: bool) {
        self.results.insert(block_hash, satisfied);
    }

    pub(super) fn remove(&mut self, block_hash: &B256) {
        self.lists.remove(block_hash);
        self.results.remove(block_hash);
        // The FIFO must stay in sync with `lists`: a hash left here would be pushed a second time
        // by a re-insert of the same block, and the stale copy would later evict the live entry.
        self.order.retain(|hash| hash != block_hash);
    }
}
#[cfg(test)]
mod inclusion_list_tests {
    use super::*;
    use alloy_consensus::{TxEip4844, TxEip7702, TxLegacy};
    use alloy_primitives::{Address, TxKind};
    use reth_ethereum_primitives::{
        EthPrimitives, Transaction as EthTransaction, TransactionSigned,
    };
    use reth_provider::{
        test_utils::{ExtendedAccount, MockEthProvider},
        StateProviderFactory,
    };
    use reth_testing_utils::generators::{self, generate_key, sign_tx_with_key_pair};

    const CHAIN_ID: u64 = 1;
    const BASE_FEE: u64 = 7;

    fn context() -> InclusionListContext {
        InclusionListContext {
            chain_id: CHAIN_ID,
            spec_id: SpecId::BOGOTA,
            base_fee_per_gas: Some(BASE_FEE),
            available_gas: 1_000_000,
            tx_gas_limit_cap: 500_000,
            max_initcode_size: revm::primitives::eip7954::MAX_INITCODE_SIZE,
            blob_gas_available: 6 * DATA_GAS_PER_BLOB,
            blob_gas_price: 1,
            max_blobs_per_tx: Some(6),
        }
    }

    fn legacy_tx(chain_id: Option<u64>, nonce: u64, gas_limit: u64) -> EthTransaction {
        EthTransaction::Legacy(TxLegacy {
            chain_id,
            nonce,
            gas_price: BASE_FEE as u128,
            gas_limit,
            to: TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            input: Default::default(),
        })
    }

    /// Signs `tx` and seeds the recovered sender with `account`.
    fn with_sender(
        tx: EthTransaction,
        account: ExtendedAccount,
    ) -> (TransactionSigned, StateProviderBox) {
        let mut rng = generators::rng();
        let signed = sign_tx_with_key_pair(generate_key(&mut rng), tx);
        let sender = signed.try_recover().expect("signature is valid");

        let provider = MockEthProvider::default();
        provider.add_account(sender, account);
        let state = provider.latest().expect("mock provider always has a latest state");

        (signed, state)
    }

    /// Funds the sender so that only the condition under test can reject.
    fn funded(nonce: u64) -> ExtendedAccount {
        ExtendedAccount::new(nonce, U256::from(10u64).pow(U256::from(20u64)))
    }

    fn could_append(
        tx: EthTransaction,
        account: ExtendedAccount,
        ctx: InclusionListContext,
    ) -> bool {
        let (signed, state) = with_sender(tx, account);
        could_append_transaction::<EthPrimitives>(&signed, &state, &ctx, &AddressMap::default())
            .expect("mock state provider does not fail")
    }

    #[test]
    fn eligible_transaction_is_appendable() {
        assert!(could_append(legacy_tx(Some(CHAIN_ID), 0, 100_000), funded(0), context()));
    }

    #[test]
    fn legacy_transaction_without_chain_id_is_appendable() {
        // A pre-EIP-155 transaction is replay-protected by omission, not by mismatch.
        assert!(could_append(legacy_tx(None, 0, 100_000), funded(0), context()));
    }

    #[test]
    fn foreign_chain_id_is_not_appendable() {
        assert!(!could_append(legacy_tx(Some(CHAIN_ID + 1), 0, 100_000), funded(0), context()));
    }

    #[test]
    fn nonce_mismatch_is_not_appendable() {
        // Too high: the sender has not reached this nonce yet.
        assert!(!could_append(legacy_tx(Some(CHAIN_ID), 5, 100_000), funded(0), context()));
        // Too low: the nonce has already been consumed.
        assert!(!could_append(legacy_tx(Some(CHAIN_ID), 0, 100_000), funded(5), context()));
    }

    #[test]
    fn max_nonce_is_not_appendable() {
        // EIP-2681 reserves the maximum uint64 nonce.
        assert!(!could_append(
            legacy_tx(Some(CHAIN_ID), u64::MAX, 100_000),
            funded(u64::MAX),
            context()
        ));
    }

    #[test]
    fn insufficient_balance_is_not_appendable() {
        let account = ExtendedAccount::new(0, U256::from(1u64));
        assert!(!could_append(legacy_tx(Some(CHAIN_ID), 0, 100_000), account, context()));
    }

    #[test]
    fn exceeding_remaining_block_gas_is_not_appendable() {
        let ctx = InclusionListContext { available_gas: 50_000, ..context() };
        assert!(!could_append(legacy_tx(Some(CHAIN_ID), 0, 100_000), funded(0), ctx));
    }

    #[test]
    fn gas_limit_over_the_cap_is_still_appendable() {
        // EIP-8037 caps regular gas, not the gas limit. A simple transfer's intrinsic gas fits
        // under the cap, so a limit above it stays appendable.
        let ctx = InclusionListContext {
            available_gas: 30_000_000,
            tx_gas_limit_cap: 100_000,
            ..context()
        };
        assert!(could_append(legacy_tx(Some(CHAIN_ID), 0, 200_000), funded(0), ctx));
    }

    #[test]
    fn intrinsic_regular_gas_over_the_cap_is_not_appendable() {
        // A cap below the 21000 intrinsic floor cannot be satisfied by any transaction.
        let ctx = InclusionListContext {
            available_gas: 30_000_000,
            tx_gas_limit_cap: 1_000,
            ..context()
        };
        assert!(!could_append(legacy_tx(Some(CHAIN_ID), 0, 200_000), funded(0), ctx));
    }

    #[test]
    fn below_intrinsic_gas_is_not_appendable() {
        assert!(!could_append(legacy_tx(Some(CHAIN_ID), 0, 1), funded(0), context()));
    }

    #[test]
    fn below_base_fee_is_not_appendable() {
        let ctx = InclusionListContext { base_fee_per_gas: Some(BASE_FEE + 1), ..context() };
        assert!(!could_append(legacy_tx(Some(CHAIN_ID), 0, 100_000), funded(0), ctx));
    }

    /// A versioned hash the KZG check accepts.
    fn kzg_hash(seed: u8) -> B256 {
        let mut hash = B256::repeat_byte(seed);
        hash.0[0] = VERSIONED_HASH_VERSION_KZG;
        hash
    }

    fn blob_tx(blob_versioned_hashes: Vec<B256>, max_fee_per_blob_gas: u128) -> EthTransaction {
        EthTransaction::Eip4844(TxEip4844 {
            chain_id: CHAIN_ID,
            nonce: 0,
            gas_limit: 100_000,
            max_fee_per_gas: BASE_FEE as u128,
            max_priority_fee_per_gas: 0,
            to: Address::ZERO,
            value: U256::ZERO,
            access_list: Default::default(),
            blob_versioned_hashes,
            max_fee_per_blob_gas,
            input: Default::default(),
        })
    }

    // The structural guards in `could_append_transaction` exist because decoding does not enforce
    // them: a non-conforming consensus layer can hand us these and they decode cleanly. Without
    // the guards an invalid transaction could be judged appendable, wrongly reporting an honest
    // block as unsatisfied.
    #[test]
    fn decoding_accepts_an_empty_authorization_list() {
        use alloy_eips::eip2718::Encodable2718;
        let mut rng = generators::rng();
        let tx = EthTransaction::Eip7702(TxEip7702 {
            chain_id: CHAIN_ID,
            nonce: 0,
            gas_limit: 100_000,
            max_fee_per_gas: BASE_FEE as u128,
            max_priority_fee_per_gas: 0,
            to: Address::ZERO,
            value: U256::ZERO,
            access_list: Default::default(),
            authorization_list: Vec::new(),
            input: Default::default(),
        });
        let encoded = sign_tx_with_key_pair(generate_key(&mut rng), tx).encoded_2718();
        assert!(TransactionSigned::decode_2718_exact(encoded.as_ref()).is_ok());
    }

    #[test]
    fn decoding_accepts_a_blob_transaction_without_blobs() {
        use alloy_eips::eip2718::Encodable2718;
        let mut rng = generators::rng();
        let encoded =
            sign_tx_with_key_pair(generate_key(&mut rng), blob_tx(Vec::new(), 1)).encoded_2718();
        assert!(TransactionSigned::decode_2718_exact(encoded.as_ref()).is_ok());
    }

    #[test]
    fn same_block_withdrawal_credit_does_not_fund_a_sender() {
        // The spec checks the list before `process_withdrawals`, so a sender funded only by a
        // withdrawal in this same block is not yet includable.
        let mut rng = generators::rng();
        let tx = legacy_tx(Some(CHAIN_ID), 0, 100_000);
        let signed = sign_tx_with_key_pair(generate_key(&mut rng), tx);
        let sender = signed.try_recover().expect("signature is valid");

        let balance = U256::from(100_000u64) * U256::from(BASE_FEE);
        let provider = MockEthProvider::default();
        provider.add_account(sender, ExtendedAccount::new(0, balance));
        let state = provider.latest().expect("mock provider always has a latest state");

        let appendable = |withdrawn: AddressMap<U256>| {
            could_append_transaction::<EthPrimitives>(&signed, &state, &context(), &withdrawn)
                .expect("mock state provider does not fail")
        };

        assert!(appendable(AddressMap::default()));
        // The whole balance arrived as a withdrawal in this block.
        assert!(!appendable(std::iter::once((sender, balance)).collect()));
    }

    #[test]
    fn well_formed_blob_transaction_is_appendable() {
        // A blob transaction is appendable like any other type: the list carries the consensus
        // form, and a proposer holding the sidecar can include it.
        assert!(could_append(blob_tx(vec![kzg_hash(1)], 1), funded(0), context()));
    }

    #[test]
    fn malformed_blob_transactions_are_not_appendable() {
        // No blobs at all, more blobs than a transaction may carry, and a hash that is not
        // KZG-versioned are each rejected by `validate_transaction`.
        assert!(!could_append(blob_tx(Vec::new(), 1), funded(0), context()));
        let too_many = (0..7).map(kzg_hash).collect::<Vec<_>>();
        assert!(!could_append(blob_tx(too_many, 1), funded(0), context()));
        assert!(!could_append(blob_tx(vec![B256::ZERO], 1), funded(0), context()));
    }

    #[test]
    fn blob_transaction_over_the_block_blob_budget_is_not_appendable() {
        let ctx = InclusionListContext { blob_gas_available: DATA_GAS_PER_BLOB - 1, ..context() };
        assert!(!could_append(blob_tx(vec![kzg_hash(1)], 1), funded(0), ctx));
    }

    #[test]
    fn blob_transaction_below_the_blob_gas_price_is_not_appendable() {
        let ctx = InclusionListContext { blob_gas_price: 2, ..context() };
        assert!(!could_append(blob_tx(vec![kzg_hash(1)], 1), funded(0), ctx));
    }

    #[test]
    fn blob_fee_counts_toward_the_senders_balance() {
        // The sender prepays blob gas at its own fee cap, so a balance that covers only the
        // execution gas is not enough.
        let tx = blob_tx(vec![kzg_hash(1)], 1_000_000_000_000);
        let execution_only = U256::from(100_000u64) * U256::from(BASE_FEE);
        assert!(!could_append(tx.clone(), ExtendedAccount::new(0, execution_only), context()));
        assert!(could_append(tx, funded(0), context()));
    }

    #[test]
    fn empty_authorization_list_is_not_appendable() {
        let tx = EthTransaction::Eip7702(TxEip7702 {
            chain_id: CHAIN_ID,
            nonce: 0,
            gas_limit: 100_000,
            max_fee_per_gas: BASE_FEE as u128,
            max_priority_fee_per_gas: 0,
            to: Address::ZERO,
            value: U256::ZERO,
            access_list: Default::default(),
            authorization_list: Vec::new(),
            input: Default::default(),
        });
        assert!(!could_append(tx, funded(0), context()));
    }

    #[test]
    fn init_code_bound_follows_the_fork() {
        // EIP-7954 raises the EIP-3860 limit in Amsterdam, so a creation whose init code falls
        // between the two limits is appendable and one above the raised limit is not.
        let create = |input_len: usize| {
            EthTransaction::Legacy(TxLegacy {
                chain_id: Some(CHAIN_ID),
                nonce: 0,
                gas_price: BASE_FEE as u128,
                gas_limit: 30_000_000,
                to: TxKind::Create,
                value: U256::ZERO,
                input: vec![0u8; input_len].into(),
            })
        };
        let ctx = InclusionListContext {
            available_gas: 30_000_000,
            tx_gas_limit_cap: 30_000_000,
            ..context()
        };

        assert!(could_append(
            create(revm::primitives::eip3860::MAX_INITCODE_SIZE + 1),
            funded(0),
            ctx
        ));
        assert!(!could_append(create(ctx.max_initcode_size + 1), funded(0), ctx));
    }

    #[test]
    fn sender_with_an_empty_code_hash_is_appendable() {
        // A sender read from state the block did not touch arrives with
        // `bytecode_hash: Some(KECCAK_EMPTY)`, which is the common case, so only the code hash
        // itself distinguishes it from a contract.
        let account = ExtendedAccount::new(0, U256::from(10u64).pow(U256::from(20u64)))
            .with_bytecode(Bytes::new());
        assert!(could_append(legacy_tx(Some(CHAIN_ID), 0, 100_000), account, context()));
    }

    #[test]
    fn contract_sender_is_not_appendable() {
        // An account carrying non-delegation code is not an EOA and cannot originate a tx.
        let account = funded(0).with_bytecode(alloy_primitives::bytes!("60006000"));
        assert!(!could_append(legacy_tx(Some(CHAIN_ID), 0, 100_000), account, context()));
    }

    #[test]
    fn reinserting_a_removed_hash_does_not_evict_it_early() {
        let mut retained = RetainedInclusionLists::default();
        let hash = B256::with_last_byte(1);

        // A payload that came back INVALID is removed, then the same block hash is submitted
        // again with a fresh list.
        retained.insert(hash, Vec::new());
        retained.remove(&hash);
        retained.insert(hash, vec![Bytes::from_static(b"tx")]);

        // The re-inserted list survives a full window of other hashes: it is the newest entry,
        // so only a 65th distinct hash may evict it.
        for i in 0..MAX_RETAINED_INCLUSION_LISTS - 1 {
            retained.insert(B256::with_last_byte(i as u8 + 2), Vec::new());
        }
        assert_eq!(retained.get(&hash), Some(&vec![Bytes::from_static(b"tx")]));
    }
}
