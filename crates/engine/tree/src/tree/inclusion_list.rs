//! EIP-7805 (FOCIL) inclusion-list handling for the engine tree.
//!
//! Covers the appendability check that decides whether a block satisfies the inclusion list it
//! was given, and the bounded store of lists retained from `engine_newPayloadV6`.

use alloy_consensus::Transaction;
use alloy_eips::eip2718::Decodable2718;
use alloy_primitives::{
    map::{B256Map, B256Set},
    Bytes, B256, U256,
};
use reth_errors::ProviderResult;
use reth_primitives_traits::{BlockBody as _, NodePrimitives, RecoveredBlock, SignedTransaction};
use reth_provider::StateProviderBox;
use revm::{
    context_interface::cfg::gas_params::Eip2780TxInfo,
    interpreter::gas::calculate_initial_tx_gas,
    primitives::{eip3860::MAX_INITCODE_SIZE, hardfork::SpecId},
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

    for encoded in transactions {
        let Ok(transaction) = N::SignedTx::decode_2718_exact(encoded) else { continue };
        if included.contains(&transaction.recalculate_hash()) {
            continue
        }
        if could_append_transaction::<N>(&transaction, state, ctx)? {
            return Ok(false)
        }
    }
    Ok(true)
}

/// Returns whether `transaction` could have been validly appended to the end of the block.
///
/// Mirrors `check_inclusion_list_transactions` in the execution spec
/// (`src/ethereum/forks/amsterdam/fork.py`). Conditions rejected here must also be ones the
/// payload builder rejects, otherwise reth reports its own blocks as unsatisfied.
fn could_append_transaction<N: NodePrimitives>(
    transaction: &N::SignedTx,
    state: &StateProviderBox,
    ctx: &InclusionListContext,
) -> ProviderResult<bool> {
    // EIP-2681 reserves the maximum nonce; execution could not increment past it.
    if transaction.nonce() == u64::MAX {
        return Ok(false)
    }

    // An inclusion list carries only EIP-2718 bytes, so the sidecar a blob transaction needs is
    // unavailable and no proposer can append one from the list. The payload builder skips them
    // for the same reason; treating them as appendable here would report our own blocks as
    // unsatisfied.
    if transaction.blob_count().is_some() {
        return Ok(false)
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

    // EIP-3860 init code bound.
    if transaction.is_create() && transaction.input().len() > MAX_INITCODE_SIZE {
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

    // An account carrying code is not an EOA unless the code is an EIP-7702 delegation.
    if account.has_bytecode() && !state.account_code(&sender)?.is_some_and(|code| code.is_eip7702())
    {
        return Ok(false)
    }

    let max_gas_cost = U256::from(transaction.gas_limit())
        .checked_mul(U256::from(transaction.max_fee_per_gas()))
        .unwrap_or(U256::MAX);
    let max_cost = max_gas_cost.checked_add(transaction.value()).unwrap_or(U256::MAX);

    Ok(account.nonce == transaction.nonce() && account.balance >= max_cost)
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
        could_append_transaction::<EthPrimitives>(&signed, &state, &ctx)
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
    fn blob_transactions_are_never_appendable() {
        // The list carries only EIP-2718 bytes, so the sidecar is unavailable and the payload
        // builder skips them. The check here has to agree, or we flag our own blocks.
        assert!(!could_append(blob_tx(vec![B256::ZERO], 1), funded(0), context()));
        assert!(!could_append(blob_tx(Vec::new(), 1), funded(0), context()));
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
