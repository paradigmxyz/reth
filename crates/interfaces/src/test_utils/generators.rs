use rand::{thread_rng, Rng};
use reth_primitives::{
    proofs, Address, BlockLocked, Bytes, Header, SealedHeader, Signature, Transaction,
    TransactionKind, TransactionSigned, TxLegacy, H256, U256,
};
use secp256k1::{KeyPair, Message as SecpMessage, Secp256k1, SecretKey};

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
    Transaction::Legacy(TxLegacy {
        chain_id: Some(1),
        nonce: rand::random::<u16>().into(),
        gas_price: rand::random::<u16>().into(),
        gas_limit: rand::random::<u16>().into(),
        to: TransactionKind::Call(Address::random()),
        value: rand::random::<u16>().into(),
        input: Bytes::default(),
    })
}

/// Generates a random legacy [Transaction] that is signed.
///
/// On top of the considerations of [gen_random_tx], these apply as well:
///
/// - There is no guarantee that the nonce is not used twice for the same account
pub fn random_signed_tx() -> TransactionSigned {
    let secp = Secp256k1::new();
    let key_pair = KeyPair::new(&secp, &mut rand::thread_rng());
    let tx = random_tx();
    let signature =
        sign_message(H256::from_slice(&key_pair.secret_bytes()[..]), tx.signature_hash()).unwrap();
    TransactionSigned::from_transaction_and_signature(tx, signature)
}

/// Signs message with the given secret key.
/// Returns the corresponding signature.
pub fn sign_message(secret: H256, message: H256) -> Result<Signature, secp256k1::Error> {
    let secp = Secp256k1::new();
    let sec = SecretKey::from_slice(secret.as_ref())?;
    let s = secp.sign_ecdsa_recoverable(&SecpMessage::from_slice(&message[..])?, &sec);
    let (rec_id, data) = s.serialize_compact();

    Ok(Signature {
        r: U256::from_big_endian(&data[..32]),
        s: U256::from_big_endian(&data[32..64]),
        odd_y_parity: rec_id.to_i32() != 0,
    })
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
pub fn random_block(number: u64, parent: Option<H256>, tx_count: Option<u8>) -> BlockLocked {
    let mut rng = thread_rng();

    // Generate transactions
    let tx_count = tx_count.unwrap_or(rand::random::<u8>());
    let transactions: Vec<TransactionSigned> =
        (0..tx_count).into_iter().map(|_| random_signed_tx()).collect();
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
            None,
        ));
    }
    blocks
}

#[cfg(test)]
mod test {
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
}
