//! Generators for different data structures like block headers, block bodies and ranges of those.

use alloy_consensus::TxLegacy;
use alloy_eips::{
    eip6110::DepositRequest, eip7002::WithdrawalRequest, eip7251::ConsolidationRequest,
};
use alloy_primitives::{Address, BlockNumber, Bytes, Parity, Sealable, TxKind, B256, U256};
pub use rand::Rng;
use rand::{
    distributions::uniform::SampleRange, rngs::StdRng, seq::SliceRandom, thread_rng, SeedableRng,
};
use reth_primitives::{
    proofs, sign_message, Account, BlockBody, Header, Log, Receipt, Request, Requests, SealedBlock,
    SealedHeader, StorageEntry, Transaction, TransactionSigned, Withdrawal, Withdrawals,
};
use secp256k1::{Keypair, Secp256k1};
use std::{
    cmp::{max, min},
    collections::{hash_map::DefaultHasher, BTreeMap},
    hash::Hasher,
    ops::{Range, RangeInclusive},
};

/// Used to pass arguments for random block generation function in tests
#[derive(Debug, Default)]
pub struct BlockParams {
    /// The parent hash of the block.
    pub parent: Option<B256>,
    /// The number of transactions in the block.
    pub tx_count: Option<u8>,
    /// The number of ommers (uncles) in the block.
    pub ommers_count: Option<u8>,
    /// The number of requests in the block.
    pub requests_count: Option<u8>,
    /// The number of withdrawals in the block.
    pub withdrawals_count: Option<u8>,
}

/// Used to pass arguments for random block generation function in tests
#[derive(Debug)]
pub struct BlockRangeParams {
    /// The parent hash of the block.
    pub parent: Option<B256>,
    /// The range of transactions in the block.
    /// If set, a random count between the range will be used.
    /// If not set, a random number of transactions will be used.
    pub tx_count: Range<u8>,
    /// The number of requests in the block.
    pub requests_count: Option<Range<u8>>,
    /// The number of withdrawals in the block.
    pub withdrawals_count: Option<Range<u8>>,
}

impl Default for BlockRangeParams {
    fn default() -> Self {
        Self {
            parent: None,
            tx_count: 0..u8::MAX / 2,
            requests_count: None,
            withdrawals_count: None,
        }
    }
}

/// Returns a random number generator that can be seeded using the `SEED` environment variable.
///
/// If `SEED` is not set, a random seed is used.
pub fn rng() -> StdRng {
    if let Ok(seed) = std::env::var("SEED") {
        let mut hasher = DefaultHasher::new();
        hasher.write(seed.as_bytes());
        StdRng::seed_from_u64(hasher.finish())
    } else {
        StdRng::from_rng(thread_rng()).expect("could not build rng")
    }
}

/// Generates a range of random [`SealedHeader`]s.
///
/// The parent hash of the first header
/// in the result will be equal to `head`.
///
/// The headers are assumed to not be correct if validated.
pub fn random_header_range<R: Rng>(
    rng: &mut R,
    range: Range<u64>,
    head: B256,
) -> Vec<SealedHeader> {
    let mut headers = Vec::with_capacity(range.end.saturating_sub(range.start) as usize);
    for idx in range {
        headers.push(random_header(
            rng,
            idx,
            Some(headers.last().map(|h: &SealedHeader| h.hash()).unwrap_or(head)),
        ));
    }
    headers
}

/// Generate a random [`SealedHeader`].
///
/// The header is assumed to not be correct if validated.
pub fn random_header<R: Rng>(rng: &mut R, number: u64, parent: Option<B256>) -> SealedHeader {
    let header = reth_primitives::Header {
        number,
        nonce: rng.gen(),
        difficulty: U256::from(rng.gen::<u32>()),
        parent_hash: parent.unwrap_or_default(),
        ..Default::default()
    };
    let sealed = header.seal_slow();
    let (header, seal) = sealed.into_parts();
    SealedHeader::new(header, seal)
}

/// Generates a random legacy [Transaction].
///
/// Every field is random, except:
///
/// - The chain ID, which is always 1
/// - The input, which is always nothing
pub fn random_tx<R: Rng>(rng: &mut R) -> Transaction {
    Transaction::Legacy(TxLegacy {
        chain_id: Some(1),
        nonce: rng.gen::<u16>().into(),
        gas_price: rng.gen::<u16>().into(),
        gas_limit: rng.gen::<u16>().into(),
        to: TxKind::Call(rng.gen()),
        value: U256::from(rng.gen::<u16>()),
        input: Bytes::default(),
    })
}

/// Generates a random legacy [Transaction] that is signed.
///
/// On top of the considerations of [`random_tx`], these apply as well:
///
/// - There is no guarantee that the nonce is not used twice for the same account
pub fn random_signed_tx<R: Rng>(rng: &mut R) -> TransactionSigned {
    let tx = random_tx(rng);
    sign_tx_with_random_key_pair(rng, tx)
}

/// Signs the [Transaction] with a random key pair.
pub fn sign_tx_with_random_key_pair<R: Rng>(rng: &mut R, tx: Transaction) -> TransactionSigned {
    let secp = Secp256k1::new();
    let key_pair = Keypair::new(&secp, rng);
    sign_tx_with_key_pair(key_pair, tx)
}

/// Signs the [Transaction] with the given key pair.
pub fn sign_tx_with_key_pair(key_pair: Keypair, tx: Transaction) -> TransactionSigned {
    let mut signature =
        sign_message(B256::from_slice(&key_pair.secret_bytes()[..]), tx.signature_hash()).unwrap();

    if matches!(tx, Transaction::Legacy(_)) {
        signature = if let Some(chain_id) = tx.chain_id() {
            signature.with_chain_id(chain_id)
        } else {
            signature.with_parity(Parity::NonEip155(signature.v().y_parity()))
        }
    }

    TransactionSigned::from_transaction_and_signature(tx, signature)
}

/// Generates a set of [Keypair]s based on the desired count.
pub fn generate_keys<R: Rng>(rng: &mut R, count: usize) -> Vec<Keypair> {
    let secp = Secp256k1::new();
    (0..count).map(|_| Keypair::new(&secp, rng)).collect()
}

/// Generate a random block filled with signed transactions (generated using
/// [`random_signed_tx`]). If no transaction count is provided, the number of transactions
/// will be random, otherwise the provided count will be used.
///
/// All fields use the default values (and are assumed to be invalid) except for:
///
/// - `parent_hash`
/// - `transactions_root`
/// - `ommers_hash`
///
/// Additionally, `gas_used` and `gas_limit` always exactly match the total `gas_limit` of all
/// transactions in the block.
///
/// The ommer headers are not assumed to be valid.
pub fn random_block<R: Rng>(rng: &mut R, number: u64, block_params: BlockParams) -> SealedBlock {
    // Generate transactions
    let tx_count = block_params.tx_count.unwrap_or_else(|| rng.gen::<u8>());
    let transactions: Vec<TransactionSigned> =
        (0..tx_count).map(|_| random_signed_tx(rng)).collect();
    let total_gas = transactions.iter().fold(0, |sum, tx| sum + tx.transaction.gas_limit());

    // Generate ommers
    let ommers_count = block_params.ommers_count.unwrap_or_else(|| rng.gen_range(0..2));
    let ommers = (0..ommers_count)
        .map(|_| random_header(rng, number, block_params.parent).unseal())
        .collect::<Vec<_>>();

    // Calculate roots
    let transactions_root = proofs::calculate_transaction_root(&transactions);
    let ommers_hash = proofs::calculate_ommers_root(&ommers);

    let requests = block_params
        .requests_count
        .map(|count| (0..count).map(|_| random_request(rng)).collect::<Vec<_>>());
    let requests_root = requests.as_ref().map(|requests| proofs::calculate_requests_root(requests));

    let withdrawals = block_params.withdrawals_count.map(|count| {
        (0..count)
            .map(|i| Withdrawal {
                amount: rng.gen(),
                index: i.into(),
                validator_index: i.into(),
                address: rng.gen(),
            })
            .collect::<Vec<_>>()
    });
    let withdrawals_root = withdrawals.as_ref().map(|w| proofs::calculate_withdrawals_root(w));

    let sealed = Header {
        parent_hash: block_params.parent.unwrap_or_default(),
        number,
        gas_used: total_gas,
        gas_limit: total_gas,
        transactions_root,
        ommers_hash,
        base_fee_per_gas: Some(rng.gen()),
        requests_root,
        withdrawals_root,
        ..Default::default()
    }
    .seal_slow();

    let (header, seal) = sealed.into_parts();

    SealedBlock {
        header: SealedHeader::new(header, seal),
        body: BlockBody {
            transactions,
            ommers,
            withdrawals: withdrawals.map(Withdrawals::new),
            requests: requests.map(Requests),
        },
    }
}

/// Generate a range of random blocks.
///
/// The parent hash of the first block
/// in the result will be equal to `head`.
///
/// See [`random_block`] for considerations when validating the generated blocks.
pub fn random_block_range<R: Rng>(
    rng: &mut R,
    block_numbers: RangeInclusive<BlockNumber>,
    block_range_params: BlockRangeParams,
) -> Vec<SealedBlock> {
    let mut blocks =
        Vec::with_capacity(block_numbers.end().saturating_sub(*block_numbers.start()) as usize);
    for idx in block_numbers {
        let tx_count = block_range_params.tx_count.clone().sample_single(rng);
        let requests_count =
            block_range_params.requests_count.clone().map(|r| r.sample_single(rng));
        let withdrawals_count =
            block_range_params.withdrawals_count.clone().map(|r| r.sample_single(rng));
        let parent = block_range_params.parent.unwrap_or_default();
        blocks.push(random_block(
            rng,
            idx,
            BlockParams {
                parent: Some(
                    blocks.last().map(|block: &SealedBlock| block.header.hash()).unwrap_or(parent),
                ),
                tx_count: Some(tx_count),
                ommers_count: None,
                requests_count,
                withdrawals_count,
            },
        ));
    }
    blocks
}

/// Collection of account and storage entry changes
pub type ChangeSet = Vec<(Address, Account, Vec<StorageEntry>)>;
type AccountState = (Account, Vec<StorageEntry>);

/// Generate a range of changesets for given blocks and accounts.
///
/// Returns a Vec of account and storage changes for each block,
/// along with the final state of all accounts and storages.
pub fn random_changeset_range<'a, R: Rng, IBlk, IAcc>(
    rng: &mut R,
    blocks: IBlk,
    accounts: IAcc,
    n_storage_changes: Range<u64>,
    key_range: Range<u64>,
) -> (Vec<ChangeSet>, BTreeMap<Address, AccountState>)
where
    IBlk: IntoIterator<Item = &'a SealedBlock>,
    IAcc: IntoIterator<Item = (Address, (Account, Vec<StorageEntry>))>,
{
    let mut state: BTreeMap<_, _> = accounts
        .into_iter()
        .map(|(addr, (acc, st))| (addr, (acc, st.into_iter().map(|e| (e.key, e.value)).collect())))
        .collect();

    let valid_addresses = state.keys().copied().collect::<Vec<_>>();

    let mut changesets = Vec::new();

    for _block in blocks {
        let mut changeset = Vec::new();
        let (from, to, mut transfer, new_entries) = random_account_change(
            rng,
            &valid_addresses,
            n_storage_changes.clone(),
            key_range.clone(),
        );

        // extract from sending account
        let (prev_from, _) = state.get_mut(&from).unwrap();
        changeset.push((from, *prev_from, Vec::new()));

        transfer = max(min(transfer, prev_from.balance), U256::from(1));
        prev_from.balance = prev_from.balance.wrapping_sub(transfer);

        // deposit in receiving account and update storage
        let (prev_to, storage): &mut (Account, BTreeMap<B256, U256>) = state.get_mut(&to).unwrap();

        let mut old_entries: Vec<_> = new_entries
            .into_iter()
            .filter_map(|entry| {
                let old = if entry.value.is_zero() {
                    let old = storage.remove(&entry.key);
                    if matches!(old, Some(U256::ZERO)) {
                        return None
                    }
                    old
                } else {
                    storage.insert(entry.key, entry.value)
                };
                Some(StorageEntry { value: old.unwrap_or(U256::ZERO), ..entry })
            })
            .collect();
        old_entries.sort_by_key(|entry| entry.key);

        changeset.push((to, *prev_to, old_entries));

        changeset.sort_by_key(|(address, _, _)| *address);

        prev_to.balance = prev_to.balance.wrapping_add(transfer);

        changesets.push(changeset);
    }

    let final_state = state
        .into_iter()
        .map(|(addr, (acc, storage))| {
            (addr, (acc, storage.into_iter().map(|v| v.into()).collect()))
        })
        .collect();
    (changesets, final_state)
}

/// Generate a random account change.
///
/// Returns two addresses, a `balance_change`, and a Vec of new storage entries.
pub fn random_account_change<R: Rng>(
    rng: &mut R,
    valid_addresses: &[Address],
    n_storage_changes: Range<u64>,
    key_range: Range<u64>,
) -> (Address, Address, U256, Vec<StorageEntry>) {
    let mut addresses = valid_addresses.choose_multiple(rng, 2).copied();

    let addr_from = addresses.next().unwrap_or_else(Address::random);
    let addr_to = addresses.next().unwrap_or_else(Address::random);

    let balance_change = U256::from(rng.gen::<u64>());

    let storage_changes = if n_storage_changes.is_empty() {
        Vec::new()
    } else {
        (0..n_storage_changes.sample_single(rng))
            .map(|_| random_storage_entry(rng, key_range.clone()))
            .collect()
    };

    (addr_from, addr_to, balance_change, storage_changes)
}

/// Generate a random storage change.
pub fn random_storage_entry<R: Rng>(rng: &mut R, key_range: Range<u64>) -> StorageEntry {
    let key = B256::new({
        let n = key_range.sample_single(rng);
        let mut m = [0u8; 32];
        m[24..32].copy_from_slice(&n.to_be_bytes());
        m
    });
    let value = U256::from(rng.gen::<u64>());

    StorageEntry { key, value }
}

/// Generate random Externally Owned Account (EOA account without contract).
pub fn random_eoa_account<R: Rng>(rng: &mut R) -> (Address, Account) {
    let nonce: u64 = rng.gen();
    let balance = U256::from(rng.gen::<u32>());
    let addr = rng.gen();

    (addr, Account { nonce, balance, bytecode_hash: None })
}

/// Generate random Externally Owned Accounts
pub fn random_eoa_accounts<R: Rng>(rng: &mut R, accounts_num: usize) -> Vec<(Address, Account)> {
    let mut accounts = Vec::with_capacity(accounts_num);
    for _ in 0..accounts_num {
        accounts.push(random_eoa_account(rng))
    }
    accounts
}

/// Generate random Contract Accounts
pub fn random_contract_account_range<R: Rng>(
    rng: &mut R,
    acc_range: &mut Range<u64>,
) -> Vec<(Address, Account)> {
    let mut accounts = Vec::with_capacity(acc_range.end.saturating_sub(acc_range.start) as usize);
    for _ in acc_range {
        let (address, eoa_account) = random_eoa_account(rng);
        // todo: can a non-eoa account have a nonce > 0?
        let account = Account { bytecode_hash: Some(rng.gen()), ..eoa_account };
        accounts.push((address, account))
    }
    accounts
}

/// Generate random receipt for transaction
pub fn random_receipt<R: Rng>(
    rng: &mut R,
    transaction: &TransactionSigned,
    logs_count: Option<u8>,
) -> Receipt {
    let success = rng.gen::<bool>();
    let logs_count = logs_count.unwrap_or_else(|| rng.gen::<u8>());
    #[allow(clippy::needless_update)] // side-effect of optimism fields
    Receipt {
        tx_type: transaction.tx_type(),
        success,
        cumulative_gas_used: rng.gen_range(0..=transaction.gas_limit()),
        logs: if success {
            (0..logs_count).map(|_| random_log(rng, None, None)).collect()
        } else {
            vec![]
        },
        ..Default::default()
    }
}

/// Generate random log
pub fn random_log<R: Rng>(rng: &mut R, address: Option<Address>, topics_count: Option<u8>) -> Log {
    let data_byte_count = rng.gen::<u8>() as usize;
    let topics_count = topics_count.unwrap_or_else(|| rng.gen()) as usize;
    Log::new_unchecked(
        address.unwrap_or_else(|| rng.gen()),
        std::iter::repeat_with(|| rng.gen()).take(topics_count).collect(),
        std::iter::repeat_with(|| rng.gen()).take(data_byte_count).collect::<Vec<_>>().into(),
    )
}

/// Generate random request
pub fn random_request<R: Rng>(rng: &mut R) -> Request {
    let request_type = rng.gen_range(0..3);
    match request_type {
        0 => Request::DepositRequest(DepositRequest {
            pubkey: rng.gen(),
            withdrawal_credentials: rng.gen(),
            amount: rng.gen(),
            signature: rng.gen(),
            index: rng.gen(),
        }),
        1 => Request::WithdrawalRequest(WithdrawalRequest {
            source_address: rng.gen(),
            validator_pubkey: rng.gen(),
            amount: rng.gen(),
        }),
        2 => Request::ConsolidationRequest(ConsolidationRequest {
            source_address: rng.gen(),
            source_pubkey: rng.gen(),
            target_pubkey: rng.gen(),
        }),
        _ => panic!("invalid request type"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::TxEip1559;
    use alloy_eips::eip2930::AccessList;
    use alloy_primitives::{hex, Parity};
    use reth_primitives::{public_key_to_address, Signature};
    use std::str::FromStr;

    #[test]
    fn test_sign_message() {
        let secp = Secp256k1::new();

        let tx = Transaction::Eip1559(TxEip1559 {
            chain_id: 1,
            nonce: 0x42,
            gas_limit: 44386,
            to: TxKind::Call(hex!("6069a6c32cf691f5982febae4faf8a6f3ab2f0f6").into()),
            value: U256::from(0_u64),
            input:  hex!("a22cb4650000000000000000000000005eee75727d804a2b13038928d36f8b188945a57a0000000000000000000000000000000000000000000000000000000000000000").into(),
            max_fee_per_gas: 0x4a817c800,
            max_priority_fee_per_gas: 0x3b9aca00,
            access_list: AccessList::default(),
        });
        let signature_hash = tx.signature_hash();

        for _ in 0..100 {
            let key_pair = Keypair::new(&secp, &mut rand::thread_rng());

            let signature =
                sign_message(B256::from_slice(&key_pair.secret_bytes()[..]), signature_hash)
                    .unwrap();

            let signed = TransactionSigned::from_transaction_and_signature(tx.clone(), signature);
            let recovered = signed.recover_signer().unwrap();

            let expected = public_key_to_address(key_pair.public_key());
            assert_eq!(recovered, expected);
        }
    }

    #[test]
    fn test_sign_eip_155() {
        // reference: https://github.com/ethereum/EIPs/blob/master/EIPS/eip-155.md#example
        let transaction = Transaction::Legacy(TxLegacy {
            chain_id: Some(1),
            nonce: 9,
            gas_price: 20 * 10_u128.pow(9),
            gas_limit: 21000,
            to: TxKind::Call(hex!("3535353535353535353535353535353535353535").into()),
            value: U256::from(10_u128.pow(18)),
            input: Bytes::default(),
        });

        // TODO resolve dependency issue
        // let expected =
        // hex!("ec098504a817c800825208943535353535353535353535353535353535353535880de0b6b3a764000080018080");
        // assert_eq!(expected, &alloy_rlp::encode(transaction));

        let hash = transaction.signature_hash();
        let expected =
            B256::from_str("daf5a779ae972f972197303d7b574746c7ef83eadac0f2791ad23db92e4c8e53")
                .unwrap();
        assert_eq!(expected, hash);

        let secret =
            B256::from_str("4646464646464646464646464646464646464646464646464646464646464646")
                .unwrap();
        let signature = sign_message(secret, hash).unwrap();

        let expected = Signature::new(
            U256::from_str(
                "18515461264373351373200002665853028612451056578545711640558177340181847433846",
            )
            .unwrap(),
            U256::from_str(
                "46948507304638947509940763649030358759909902576025900602547168820602576006531",
            )
            .unwrap(),
            Parity::Parity(false),
        );
        assert_eq!(expected, signature);
    }
}
