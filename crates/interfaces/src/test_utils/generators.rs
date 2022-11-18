use rand::{thread_rng, Rng};
use reth_primitives::{
    proofs, Address, BlockLocked, Bytes, Header, SealedHeader, Signature, Transaction,
    TransactionKind, TransactionSigned, H256, U256,
};

// TODO(onbjerg): Maybe we should split this off to its own crate, or move the helpers to the
// relevant crates?

/// Generates a range of random [SealedHeader]s.
///
/// The parent hash of the first header
/// in the result will be equal to `head`.
///
/// The headers are assumed to not be correct if validated.
pub fn random_header_range(rng: std::ops::Range<u64>, head: H256) -> Vec<SealedHeader> {
    let mut headers = Vec::with_capacity(rng.end.saturating_sub(rng.start) as usize);
    for idx in rng {
        headers.push(random_header(
            idx,
            Some(headers.last().map(|h: &SealedHeader| h.hash()).unwrap_or(head)),
        ));
    }
    headers
}

/// Generate a random [SealedHeader].
///
/// The header is assumed to not be correct if validated.
pub fn random_header(number: u64, parent: Option<H256>) -> SealedHeader {
    let header = reth_primitives::Header {
        number,
        nonce: rand::random(),
        difficulty: U256::from(rand::random::<u32>()),
        parent_hash: parent.unwrap_or_default(),
        ..Default::default()
    };
    header.seal()
}

/// Generates a random legacy [Transaction].
///
/// Every field is random, except:
///
/// - The chain ID, which is always 1
/// - The input, which is always nothing
pub fn random_tx() -> Transaction {
    Transaction::Legacy {
        chain_id: Some(1),
        nonce: rand::random::<u16>().into(),
        gas_price: rand::random::<u16>().into(),
        gas_limit: rand::random::<u16>().into(),
        to: TransactionKind::Call(Address::random()),
        value: rand::random::<u16>().into(),
        input: Bytes::default(),
    }
}

/// Generates a random legacy [Transaction] that is signed.
///
/// On top of the considerations of [gen_random_tx], these apply as well:
///
/// - There is no guarantee that the nonce is not used twice for the same account
pub fn random_signed_tx() -> TransactionSigned {
    let tx = random_tx();
    let hash = tx.signature_hash();
    TransactionSigned {
        transaction: tx,
        hash,
        signature: Signature {
            // TODO
            r: Default::default(),
            s: Default::default(),
            odd_y_parity: false,
        },
    }
}

/// Generate a random block filled with a random number of signed transactions (generated using
/// [random_signed_tx]).
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
pub fn random_block(number: u64, parent: Option<H256>) -> BlockLocked {
    let mut rng = thread_rng();

    // Generate transactions
    let transactions: Vec<TransactionSigned> =
        (0..rand::random::<u8>()).into_iter().map(|_| random_signed_tx()).collect();
    let total_gas = transactions.iter().fold(0, |sum, tx| sum + tx.transaction.gas_limit());

    // Generate ommers
    let mut ommers = Vec::new();
    for _ in 0..rng.gen_range(0..2) {
        ommers.push(random_header(number, parent).unseal());
    }

    // Calculate roots
    let transactions_root = proofs::calculate_transaction_root(transactions.iter());
    let ommers_hash = proofs::calculate_ommers_root(ommers.iter());

    BlockLocked {
        header: Header {
            parent_hash: parent.unwrap_or_default(),
            number,
            gas_used: total_gas,
            gas_limit: total_gas,
            transactions_root,
            ommers_hash,
            ..Default::default()
        }
        .seal(),
        body: transactions,
        ommers: ommers.into_iter().map(|ommer| ommer.seal()).collect(),
    }
}

/// Generate a range of random blocks.
///
/// The parent hash of the first block
/// in the result will be equal to `head`.
///
/// See [random_block] for considerations when validating the generated blocks.
pub fn random_block_range(rng: std::ops::Range<u64>, head: H256) -> Vec<BlockLocked> {
    let mut blocks = Vec::with_capacity(rng.end.saturating_sub(rng.start) as usize);
    for idx in rng {
        blocks.push(random_block(
            idx,
            Some(blocks.last().map(|block: &BlockLocked| block.header.hash()).unwrap_or(head)),
        ));
    }
    blocks
}
