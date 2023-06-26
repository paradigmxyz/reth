pub use rand::Rng;
use rand::{
    distributions::uniform::SampleRange, rngs::StdRng, seq::SliceRandom, thread_rng, SeedableRng,
};
use reth_primitives::{
    proofs, sign_message, Account, Address, BlockNumber, Bytes, Header, Log, Receipt, SealedBlock,
    SealedHeader, Signature, StorageEntry, Transaction, TransactionKind, TransactionSigned,
    TxLegacy, H160, H256, U256,
};
use secp256k1::{KeyPair, Message as SecpMessage, Secp256k1, SecretKey, SECP256K1};
use std::{
    cmp::{max, min},
    collections::{hash_map::DefaultHasher, BTreeMap},
    hash::Hasher,
    ops::{Range, RangeInclusive, Sub},
};

// TODO(onbjerg): Maybe we should split this off to its own crate, or move the helpers to the
// relevant crates?

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

/// Generates a range of random [SealedHeader]s.
///
/// The parent hash of the first header
/// in the result will be equal to `head`.
///
/// The headers are assumed to not be correct if validated.
pub fn random_header_range<R: Rng>(
    rng: &mut R,
    range: std::ops::Range<u64>,
    head: H256,
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

/// Generate a random [SealedHeader].
///
/// The header is assumed to not be correct if validated.
pub fn random_header<R: Rng>(rng: &mut R, number: u64, parent: Option<H256>) -> SealedHeader {
    let header = reth_primitives::Header {
        number,
        nonce: rng.gen(),
        difficulty: U256::from(rng.gen::<u32>()),
        parent_hash: parent.unwrap_or_default(),
        ..Default::default()
    };
    header.seal_slow()
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
        to: TransactionKind::Call(Address::random()),
        value: rng.gen::<u16>().into(),
        input: Bytes::default(),
    })
}

/// Generates a random legacy [Transaction] that is signed.
///
/// On top of the considerations of [random_tx], these apply as well:
///
/// - There is no guarantee that the nonce is not used twice for the same account
pub fn random_signed_tx<R: Rng>(rng: &mut R) -> TransactionSigned {
    let secp = Secp256k1::new();
    let key_pair = KeyPair::new(&secp, rng);
    let tx = random_tx(rng);
    sign_tx_with_key_pair(key_pair, tx)
}

/// Signs the [Transaction] with the given key pair.
pub fn sign_tx_with_key_pair(key_pair: KeyPair, tx: Transaction) -> TransactionSigned {
    let signature =
        sign_message(H256::from_slice(&key_pair.secret_bytes()[..]), tx.signature_hash()).unwrap();
    TransactionSigned::from_transaction_and_signature(tx, signature)
}

/// Generate a random block filled with signed transactions (generated using
/// [random_signed_tx]). If no transaction count is provided, the number of transactions
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
pub fn random_block<R: Rng>(
    rng: &mut R,
    number: u64,
    parent: Option<H256>,
    tx_count: Option<u8>,
    ommers_count: Option<u8>,
) -> SealedBlock {
    // Generate transactions
    let tx_count = tx_count.unwrap_or_else(|| rng.gen::<u8>());
    let transactions: Vec<TransactionSigned> =
        (0..tx_count).map(|_| random_signed_tx(rng)).collect();
    let total_gas = transactions.iter().fold(0, |sum, tx| sum + tx.transaction.gas_limit());

    // Generate ommers
    let ommers_count = ommers_count.unwrap_or_else(|| rng.gen_range(0..2));
    let ommers =
        (0..ommers_count).map(|_| random_header(rng, number, parent).unseal()).collect::<Vec<_>>();

    // Calculate roots
    let transactions_root = proofs::calculate_transaction_root(&transactions);
    let ommers_hash = proofs::calculate_ommers_root(&ommers);

    SealedBlock {
        header: Header {
            parent_hash: parent.unwrap_or_default(),
            number,
            gas_used: total_gas,
            gas_limit: total_gas,
            transactions_root,
            ommers_hash,
            base_fee_per_gas: Some(rng.gen()),
            ..Default::default()
        }
        .seal_slow(),
        body: transactions,
        ommers,
        withdrawals: None,
    }
}

/// Generate a range of random blocks.
///
/// The parent hash of the first block
/// in the result will be equal to `head`.
///
/// See [random_block] for considerations when validating the generated blocks.
pub fn random_block_range<R: Rng>(
    rng: &mut R,
    block_numbers: RangeInclusive<BlockNumber>,
    head: H256,
    tx_count: Range<u8>,
) -> Vec<SealedBlock> {
    let mut blocks =
        Vec::with_capacity(block_numbers.end().saturating_sub(*block_numbers.start()) as usize);
    for idx in block_numbers {
        let tx_count = tx_count.clone().sample_single(rng);
        blocks.push(random_block(
            rng,
            idx,
            Some(blocks.last().map(|block: &SealedBlock| block.header.hash()).unwrap_or(head)),
            Some(tx_count),
            None,
        ));
    }
    blocks
}

type Transition = Vec<(Address, Account, Vec<StorageEntry>)>;
type AccountState = (Account, Vec<StorageEntry>);

/// Generate a range of transitions for given blocks and accounts.
/// Assumes all accounts start with an empty storage.
///
/// Returns a Vec of account and storage changes for each transition,
/// along with the final state of all accounts and storages.
pub fn random_transition_range<'a, R: Rng, IBlk, IAcc>(
    rng: &mut R,
    blocks: IBlk,
    accounts: IAcc,
    n_changes: std::ops::Range<u64>,
    key_range: std::ops::Range<u64>,
) -> (Vec<Transition>, BTreeMap<Address, AccountState>)
where
    IBlk: IntoIterator<Item = &'a SealedBlock>,
    IAcc: IntoIterator<Item = (Address, (Account, Vec<StorageEntry>))>,
{
    let mut state: BTreeMap<_, _> = accounts
        .into_iter()
        .map(|(addr, (acc, st))| (addr, (acc, st.into_iter().map(|e| (e.key, e.value)).collect())))
        .collect();

    let valid_addresses = state.keys().copied().collect();

    let mut transitions = Vec::new();

    blocks.into_iter().for_each(|block| {
        let mut transition = Vec::new();
        let (from, to, mut transfer, new_entries) =
            random_account_change(rng, &valid_addresses, n_changes.clone(), key_range.clone());

        // extract from sending account
        let (prev_from, _) = state.get_mut(&from).unwrap();
        transition.push((from, *prev_from, Vec::new()));

        transfer = max(min(transfer, prev_from.balance), U256::from(1));
        prev_from.balance = prev_from.balance.wrapping_sub(transfer);

        // deposit in receiving account and update storage
        let (prev_to, storage): &mut (Account, BTreeMap<H256, U256>) = state.get_mut(&to).unwrap();

        let old_entries = new_entries
            .into_iter()
            .filter_map(|entry| {
                let old = if entry.value != U256::ZERO {
                    storage.insert(entry.key, entry.value)
                } else {
                    let old = storage.remove(&entry.key);
                    if matches!(old, Some(U256::ZERO)) {
                        return None
                    }
                    old
                };
                Some(StorageEntry { value: old.unwrap_or(U256::from(0)), ..entry })
            })
            .collect();

        transition.push((to, *prev_to, old_entries));

        prev_to.balance = prev_to.balance.wrapping_add(transfer);

        transitions.push(transition);
    });

    let final_state = state
        .into_iter()
        .map(|(addr, (acc, storage))| {
            (addr, (acc, storage.into_iter().map(|v| v.into()).collect()))
        })
        .collect();
    (transitions, final_state)
}

/// Generate a random account change.
///
/// Returns two addresses, a balance_change, and a Vec of new storage entries.
pub fn random_account_change<R: Rng>(
    rng: &mut R,
    valid_addresses: &Vec<Address>,
    n_changes: std::ops::Range<u64>,
    key_range: std::ops::Range<u64>,
) -> (Address, Address, U256, Vec<StorageEntry>) {
    let mut addresses = valid_addresses.choose_multiple(rng, 2).cloned();

    let addr_from = addresses.next().unwrap_or_else(Address::random);
    let addr_to = addresses.next().unwrap_or_else(Address::random);

    let balance_change = U256::from(rng.gen::<u64>());

    let storage_changes = (0..n_changes.sample_single(rng))
        .map(|_| random_storage_entry(rng, key_range.clone()))
        .collect();

    (addr_from, addr_to, balance_change, storage_changes)
}

/// Generate a random storage change.
pub fn random_storage_entry<R: Rng>(rng: &mut R, key_range: std::ops::Range<u64>) -> StorageEntry {
    let key = H256::from_low_u64_be(key_range.sample_single(rng));
    let value = U256::from(rng.gen::<u64>());

    StorageEntry { key, value }
}

/// Generate random Externally Owned Account (EOA account without contract).
pub fn random_eoa_account<R: Rng>(rng: &mut R) -> (Address, Account) {
    let nonce: u64 = rng.gen();
    let balance = U256::from(rng.gen::<u32>());
    let addr = H160::from(rng.gen::<u64>());

    (addr, Account { nonce, balance, bytecode_hash: None })
}

/// Generate random Externally Owned Accounts
pub fn random_eoa_account_range<R: Rng>(
    rng: &mut R,
    acc_range: std::ops::Range<u64>,
) -> Vec<(Address, Account)> {
    let mut accounts = Vec::with_capacity(acc_range.end.saturating_sub(acc_range.start) as usize);
    for _ in acc_range {
        accounts.push(random_eoa_account(rng))
    }
    accounts
}

/// Generate random Contract Accounts
pub fn random_contract_account_range<R: Rng>(
    rng: &mut R,
    acc_range: &mut std::ops::Range<u64>,
) -> Vec<(Address, Account)> {
    let mut accounts = Vec::with_capacity(acc_range.end.saturating_sub(acc_range.start) as usize);
    for _ in acc_range {
        let (address, eoa_account) = random_eoa_account(rng);
        let account = Account { bytecode_hash: Some(H256::random()), ..eoa_account };
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
    Receipt {
        tx_type: transaction.tx_type(),
        success,
        cumulative_gas_used: rng.gen_range(0..=transaction.gas_limit()),
        logs: if success {
            (0..logs_count).map(|_| random_log(rng, None, None)).collect()
        } else {
            vec![]
        },
    }
}

/// Generate random log
pub fn random_log<R: Rng>(rng: &mut R, address: Option<Address>, topics_count: Option<u8>) -> Log {
    let data_byte_count = rng.gen::<u8>();
    let topics_count = topics_count.unwrap_or_else(|| rng.gen::<u8>());
    Log {
        address: address.unwrap_or_else(|| rng.gen()),
        topics: (0..topics_count).map(|_| rng.gen()).collect(),
        data: Bytes::from((0..data_byte_count).map(|_| rng.gen::<u8>()).collect::<Vec<_>>()),
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use super::*;
    use hex_literal::hex;
    use reth_primitives::{keccak256, AccessList, Address, TransactionKind, TxEip1559};
    use secp256k1::KeyPair;

    #[test]
    fn test_sign_message() {
        let secp = Secp256k1::new();

        let tx = Transaction::Eip1559(TxEip1559 {
            chain_id: 1,
            nonce: 0x42,
            gas_limit: 44386,
            to: TransactionKind::Call(hex!("6069a6c32cf691f5982febae4faf8a6f3ab2f0f6").into()),
            value: 0_u128,
            input:  hex!("a22cb4650000000000000000000000005eee75727d804a2b13038928d36f8b188945a57a0000000000000000000000000000000000000000000000000000000000000000").into(),
            max_fee_per_gas: 0x4a817c800,
            max_priority_fee_per_gas: 0x3b9aca00,
            access_list: AccessList::default(),
        });
        let signature_hash = tx.signature_hash();

        for _ in 0..100 {
            let key_pair = KeyPair::new(&secp, &mut rand::thread_rng());

            let signature =
                sign_message(H256::from_slice(&key_pair.secret_bytes()[..]), signature_hash)
                    .unwrap();

            let signed = TransactionSigned::from_transaction_and_signature(tx.clone(), signature);
            let recovered = signed.recover_signer().unwrap();

            let public_key_hash = keccak256(&key_pair.public_key().serialize_uncompressed()[1..]);
            let expected = Address::from_slice(&public_key_hash[12..]);

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
            to: TransactionKind::Call(hex!("3535353535353535353535353535353535353535").into()),
            value: 10_u128.pow(18),
            input: Bytes::default(),
        });

        // TODO resolve dependency issue
        // let mut encoded = BytesMut::new();
        // transaction.encode(&mut encoded);
        // let expected =
        // hex!("ec098504a817c800825208943535353535353535353535353535353535353535880de0b6b3a764000080018080");
        // assert_eq!(expected, encoded.as_ref());

        let hash = transaction.signature_hash();
        let expected =
            H256::from_str("daf5a779ae972f972197303d7b574746c7ef83eadac0f2791ad23db92e4c8e53")
                .unwrap();
        assert_eq!(expected, hash);

        let secret =
            H256::from_str("4646464646464646464646464646464646464646464646464646464646464646")
                .unwrap();
        let signature = sign_message(secret, hash).unwrap();

        let expected = Signature {
            r: U256::from_str(
                "18515461264373351373200002665853028612451056578545711640558177340181847433846",
            )
            .unwrap(),
            s: U256::from_str(
                "46948507304638947509940763649030358759909902576025900602547168820602576006531",
            )
            .unwrap(),
            odd_y_parity: false,
        };
        assert_eq!(expected, signature);
    }
}
