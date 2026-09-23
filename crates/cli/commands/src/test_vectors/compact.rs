use alloy_eips::eip4895::Withdrawals;
use alloy_primitives::{hex, Address, Bytes, Signature, TxKind, B256, U256};
use alloy_rlp::{Decodable, Encodable, Header as RlpHeader};
use arbitrary::Arbitrary;
use eyre::{Context, Result};
use proptest::{
    prelude::{ProptestConfig, RngCore},
    test_runner::{TestRng, TestRunner},
};
use reth_codecs::alloy::{
    authorization_list::Authorization,
    genesis_account::GenesisAccount,
    header::{Header, HeaderExt},
    transaction::{
        eip1559::TxEip1559, eip2930::TxEip2930, eip4844::TxEip4844, eip7702::TxEip7702,
        legacy::TxLegacy,
    },
    withdrawal::Withdrawal,
};
use reth_db::{
    models::{
        AccountBeforeTx, StaticFileBlockWithdrawals, StoredBlockBodyIndices, StoredBlockOmmers,
        StoredBlockWithdrawals,
    },
    ClientVersion,
};
use reth_ethereum_primitives::{EthereumReceipt, Transaction, TransactionSigned, TxType};
use reth_fs_util as fs;
use reth_primitives_traits::{Account, Log, LogData, StorageEntry};
use reth_prune_types::{PruneCheckpoint, PruneMode};
use reth_stages_types::{
    AccountHashingCheckpoint, CheckpointBlockRange, EntitiesCheckpoint, ExecutionCheckpoint,
    HeadersCheckpoint, IndexHistoryCheckpoint, StageCheckpoint, StageUnitCheckpoint,
    StorageHashingCheckpoint,
};
use reth_trie::{hash_builder::HashBuilderValue, TrieMask};
use reth_trie_common::{hash_builder::HashBuilderState, StoredNibbles, StoredNibblesSubKey};
use std::{fs::File, io::BufReader};

pub const VECTORS_FOLDER: &str = "testdata/micro/compact";
pub const VECTOR_SIZE: usize = 100;

#[macro_export]
macro_rules! compact_types {
    (regular: [$($regular_ty:ident),*], identifier: [$($id_ty:ident),*]) => {
        pub const GENERATE_VECTORS: &[fn(&mut TestRunner) -> eyre::Result<()>] = &[
            $(
                generate_vector::<$regular_ty> as fn(&mut TestRunner) -> eyre::Result<()>,
            )*
            $(
                generate_vector::<$id_ty> as fn(&mut TestRunner) -> eyre::Result<()>,
            )*
        ];

        pub const READ_VECTORS: &[fn() -> eyre::Result<()>] = &[
            $(
                read_vector::<$regular_ty> as fn() -> eyre::Result<()>,
            )*
            $(
                read_vector::<$id_ty> as fn() -> eyre::Result<()>,
            )*
        ];

        pub static IDENTIFIER_TYPE: std::sync::LazyLock<std::collections::HashSet<String>> = std::sync::LazyLock::new(|| {
            let mut map = std::collections::HashSet::new();
            $(
                 map.insert(type_name::<$id_ty>());
            )*
            map
        });
    };
}

// The type that **actually** implements `Compact` should go here. If it's an alloy type, import the
// auxiliary type from reth_codecs::alloy instead.
compact_types!(
    regular: [
        // reth-primitives
        Account,
        EthereumReceipt,
        // reth_codecs::alloy
        Authorization,
        GenesisAccount,
        Header,
        HeaderExt,
        Withdrawal,
        Withdrawals,
        TxEip2930,
        TxEip1559,
        TxEip4844,
        TxEip7702,
        TxLegacy,
        HashBuilderValue,
        LogData,
        Log,
        // BranchNodeCompact, // todo requires arbitrary
        TrieMask,
        // reth_prune_types
        PruneCheckpoint,
        PruneMode,
        // reth_stages_types
        AccountHashingCheckpoint,
        StorageHashingCheckpoint,
        ExecutionCheckpoint,
        HeadersCheckpoint,
        IndexHistoryCheckpoint,
        EntitiesCheckpoint,
        CheckpointBlockRange,
        StageCheckpoint,
        StageUnitCheckpoint,
        // reth_db_api
        StoredBlockOmmers,
        StoredBlockBodyIndices,
        StoredBlockWithdrawals,
        StaticFileBlockWithdrawals,
        // Manual implementations
        TransactionSigned,
        // Bytecode, // todo bytecode arbitrary
        StorageEntry,
        // MerkleCheckpoint, // todo storedsubnode -> branchnodecompact arbitrary
        AccountBeforeTx,
        ClientVersion,
        StoredNibbles,
        StoredNibblesSubKey,
        // StorageTrieEntry, // todo branchnodecompact arbitrary
        // StoredSubNode, // todo branchnodecompact arbitrary
        HashBuilderState
    ],
    // These types require an extra identifier which is usually stored elsewhere (eg. parent type).
    identifier: [
        Signature,
        Transaction,
        TxType,
        TxKind
    ]
);

/// Generates a vector of type `T` to a file.
pub fn generate_vectors() -> Result<()> {
    generate_vectors_with(GENERATE_VECTORS)
}

pub fn read_vectors() -> Result<()> {
    read_vectors_with(READ_VECTORS)
}

/// Generates a vector of type `T` to a file.
pub fn generate_vectors_with(generator: &[fn(&mut TestRunner) -> eyre::Result<()>]) -> Result<()> {
    // Prepare random seed for test (same method as used by proptest)
    let seed = B256::random();
    println!("Seed for compact test vectors: {:?}", hex::encode_prefixed(seed));

    // Start the runner with the seed
    let config = ProptestConfig::default();
    let rng = TestRng::from_seed(config.rng_algorithm, &seed.0);
    let mut runner = TestRunner::new_with_rng(config, rng);

    fs::create_dir_all(VECTORS_FOLDER)?;

    for generate_fn in generator {
        generate_fn(&mut runner)?;
    }

    Ok(())
}

/// Reads multiple vectors of different types ensuring their correctness by decoding and
/// re-encoding.
pub fn read_vectors_with(read: &[fn() -> eyre::Result<()>]) -> Result<()> {
    fs::create_dir_all(VECTORS_FOLDER)?;
    let mut errors = None;

    for read_fn in read {
        if let Err(err) = read_fn() {
            errors.get_or_insert_with(Vec::new).push(err);
        }
    }

    if let Some(err_list) = errors {
        for error in err_list {
            eprintln!("{error:?}");
        }
        return Err(eyre::eyre!(
            "If there are missing types, make sure to run `reth test-vectors compact --write` first.\n
             If it happened during CI, ignore IF it's a new proposed type that `main` branch does not have."
        ));
    }

    Ok(())
}

/// Generates test vectors for a specific type `T`.
pub fn generate_vector<T>(runner: &mut TestRunner) -> Result<()>
where
    T: for<'a> Arbitrary<'a> + reth_codecs::Compact,
{
    let type_name = type_name::<T>();
    print!("{}", type_name);

    let mut bytes = std::iter::repeat_n(0u8, 256).collect::<Vec<u8>>();
    let mut compact_buffer = vec![];

    let mut values = Vec::with_capacity(VECTOR_SIZE);
    for _ in 0..VECTOR_SIZE {
        runner.rng().fill_bytes(&mut bytes);
        compact_buffer.clear();

        // Sometimes type T, might require extra arbitrary data, so we retry it a few times.
        let mut tries = 0;
        let obj = loop {
            match T::arbitrary(&mut arbitrary::Unstructured::new(&bytes)) {
                Ok(obj) => break obj,
                Err(err) => {
                    if tries < 5 && matches!(err, arbitrary::Error::NotEnoughData) {
                        tries += 1;
                        bytes.extend(std::iter::repeat_n(0u8, 256));
                    } else {
                        return Err(err)?
                    }
                }
            }
        };
        let res = obj.to_compact(&mut compact_buffer);

        if IDENTIFIER_TYPE.contains(&type_name) {
            compact_buffer.push(res as u8);
        }

        values.push(hex::encode(&compact_buffer));
    }

    serde_json::to_writer(
        std::io::BufWriter::new(
            std::fs::File::create(format!("{VECTORS_FOLDER}/{}.json", type_name)).unwrap(),
        ),
        &values,
    )?;

    println!(" ✅");

    Ok(())
}

/// Reads a vector of type `T` from a file and compares each item with its reconstructed version
/// using `T::from_compact`.
pub fn read_vector<T>() -> Result<()>
where
    T: reth_codecs::Compact,
{
    let type_name = type_name::<T>();
    print!("{}", type_name);

    // Read the file where the vectors are stored
    let file_path = format!("{VECTORS_FOLDER}/{}.json", type_name);
    let file =
        File::open(&file_path).wrap_err_with(|| format!("Failed to open vector {type_name}."))?;
    let reader = BufReader::new(file);

    let stored_values: Vec<String> = serde_json::from_reader(reader)?;
    let mut buffer = vec![];

    for hex_str in stored_values {
        let mut compact_bytes = hex::decode(hex_str)?;
        let mut identifier = None;
        buffer.clear();

        if IDENTIFIER_TYPE.contains(&type_name) {
            identifier = compact_bytes.pop().map(|b| b as usize);
        }
        let len_or_identifier = identifier.unwrap_or(compact_bytes.len());

        // Old frame transaction vectors can contain arbitrary byte strings where the current
        // consensus type requires an empty or 20-byte address (or other typed values). Such input
        // cannot be constructed as the current transaction type, so verify its complete canonical
        // RLP roundtrip rather than dropping the vector or passing it into the strict decoder.
        let legacy_frame =
            legacy_frame_vector_has_unrepresentable_fields(&type_name, &compact_bytes)?;
        if legacy_frame {
            let reconstructed =
                canonicalize_legacy_frame_vector_with_type(&type_name, &compact_bytes)?;
            assert_eq!(reconstructed, compact_bytes, "legacy frame vector mismatch {type_name}");
        } else {
            let (reconstructed, _) = T::from_compact(&compact_bytes, len_or_identifier);
            reconstructed.to_compact(&mut buffer);
            assert_eq!(buffer, compact_bytes, "mismatch {type_name}");
        }
    }

    println!(" ✅");

    Ok(())
}

/// Checks whether an old EIP-8141 compact vector contains fields that cannot be represented by the
/// current typed frame representation. The entire transaction is parsed before returning so that
/// the caller can still validate and roundtrip every legacy vector.
fn legacy_frame_vector_has_unrepresentable_fields(type_name: &str, bytes: &[u8]) -> Result<bool> {
    let Some((_, mut input)) = legacy_frame_rlp_payload(type_name, bytes)? else {
        return Ok(false);
    };

    let mut fields = take_rlp_list(&mut input)?;
    if !input.is_empty() {
        eyre::bail!("EIP-8141 compact vector has trailing bytes")
    }

    u64::decode(&mut fields)?;
    u64::decode(&mut fields)?;
    Address::decode(&mut fields)?;

    let mut unrepresentable = false;
    let mut frames = take_rlp_list(&mut fields)?;
    while !frames.is_empty() {
        let mut frame = take_rlp_list(&mut frames)?;
        let mode = u8::decode(&mut frame)?;
        if mode > 2 {
            unrepresentable = true;
        }
        u8::decode(&mut frame)?;

        let target = RlpHeader::decode_bytes(&mut frame, false)?;
        if !target.is_empty() && target.len() != Address::len_bytes() {
            unrepresentable = true;
        }

        let mut limits = take_rlp_list(&mut frame)?;
        u64::decode(&mut limits)?;
        u64::decode(&mut limits)?;
        if !limits.is_empty() || U256::decode(&mut frame).is_err() {
            eyre::bail!("invalid EIP-8141 frame fields in compact vector")
        }
        Bytes::decode(&mut frame)?;
        if !frame.is_empty() {
            eyre::bail!("EIP-8141 frame has trailing fields in compact vector")
        }
    }

    let mut signatures = take_rlp_list(&mut fields)?;
    while !signatures.is_empty() {
        let mut signature = take_rlp_list(&mut signatures)?;
        let scheme = u8::decode(&mut signature)?;
        if scheme > 2 {
            unrepresentable = true;
        }

        let signer = RlpHeader::decode_bytes(&mut signature, false)?;
        if !signer.is_empty() && signer.len() != Address::len_bytes() {
            unrepresentable = true;
        }

        let message = RlpHeader::decode_bytes(&mut signature, false)?;
        if message.len() != 0 && (message.len() != 32 || message.iter().all(|byte| *byte == 0)) {
            unrepresentable = true;
        }

        Bytes::decode(&mut signature)?;
        if !signature.is_empty() {
            eyre::bail!("EIP-8141 signature has trailing fields in compact vector")
        }
    }

    U256::decode(&mut fields)?;
    U256::decode(&mut fields)?;
    U256::decode(&mut fields)?;
    let mut blob_hashes = take_rlp_list(&mut fields)?;
    while !blob_hashes.is_empty() {
        B256::decode(&mut blob_hashes)?;
    }
    if !fields.is_empty() {
        eyre::bail!("EIP-8141 transaction has trailing fields in compact vector")
    }

    Ok(unrepresentable)
}

/// Re-encodes a legacy frame transaction's canonical RLP without imposing the newer typed field
/// constraints. This is only used for historical compact vectors whose invalid values cannot be
/// represented by `TxEip8141`; valid frame transactions still go through `Compact::from_compact`.
fn canonicalize_legacy_frame_vector_with_type(type_name: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    let Some((prefix, mut input)) = legacy_frame_rlp_payload(type_name, bytes)? else {
        eyre::bail!("expected EIP-8141 compact vector")
    };

    let mut output = prefix.to_vec();
    canonicalize_rlp_item(&mut input, &mut output)?;
    if !input.is_empty() {
        eyre::bail!("EIP-8141 compact vector has trailing bytes")
    }
    Ok(output)
}

/// Returns the compact type prefix and the RLP payload for the two frame transaction wrappers.
fn legacy_frame_rlp_payload<'a>(
    type_name: &str,
    bytes: &'a [u8],
) -> Result<Option<(&'a [u8], &'a [u8])>> {
    const FRAME_TRANSACTION_TYPE: u8 = 0x06;
    const UNSIGNED_TRANSACTION_IDENTIFIER: u8 = u8::MAX;

    let compact_prefix: &[u8] = match type_name {
        "Transaction" => &[FRAME_TRANSACTION_TYPE],
        "TransactionSigned" => &[UNSIGNED_TRANSACTION_IDENTIFIER, FRAME_TRANSACTION_TYPE],
        _ => return Ok(None),
    };
    if !bytes.starts_with(compact_prefix) {
        return Ok(None);
    }

    let Some((&transaction_type, payload)) = bytes[compact_prefix.len()..].split_first() else {
        eyre::bail!("truncated EIP-8141 compact type")
    };
    if transaction_type != FRAME_TRANSACTION_TYPE {
        return Ok(None);
    }

    let prefix_length = compact_prefix.len() + 1;
    Ok(Some((&bytes[..prefix_length], payload)))
}

/// Recursively decodes and re-encodes one RLP item, preserving byte-string payloads while
/// canonicalizing list headers.
fn canonicalize_rlp_item(input: &mut &[u8], output: &mut Vec<u8>) -> Result<()> {
    let original = *input;
    let mut header_input = original;
    let header = RlpHeader::decode(&mut header_input)?;
    let header_length = original.len() - header_input.len();
    let item_length = header_length
        .checked_add(header.payload_length)
        .ok_or_else(|| eyre::eyre!("RLP item length overflow in EIP-8141 compact vector"))?;
    let (item, remaining) = original
        .split_at_checked(item_length)
        .ok_or_else(|| eyre::eyre!("truncated RLP item in EIP-8141 compact vector"))?;
    let payload = &item[header_length..];

    if header.list {
        let mut payload = payload;
        let mut encoded_payload = Vec::new();
        while !payload.is_empty() {
            canonicalize_rlp_item(&mut payload, &mut encoded_payload)?;
        }
        RlpHeader { list: true, payload_length: encoded_payload.len() }.encode(output);
        output.extend_from_slice(&encoded_payload);
    } else {
        Bytes::copy_from_slice(payload).encode(output);
    }

    *input = remaining;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rlp_list(payload: &[u8]) -> Vec<u8> {
        let mut encoded = Vec::new();
        RlpHeader { list: true, payload_length: payload.len() }.encode(&mut encoded);
        encoded.extend_from_slice(payload);
        encoded
    }

    fn legacy_frame_vector(type_name: &str, target: &[u8]) -> Vec<u8> {
        let mut frame = Vec::new();
        0u8.encode(&mut frame);
        0u8.encode(&mut frame);
        Bytes::copy_from_slice(target).encode(&mut frame);
        frame.extend(rlp_list(&[0x80, 0x80]));
        U256::ZERO.encode(&mut frame);
        Bytes::new().encode(&mut frame);

        let mut frames = Vec::new();
        frames.extend(rlp_list(&frame));

        let mut transaction = Vec::new();
        1u64.encode(&mut transaction);
        0u64.encode(&mut transaction);
        Address::ZERO.encode(&mut transaction);
        transaction.extend(rlp_list(&frames));
        transaction.extend(rlp_list(&[]));
        U256::ZERO.encode(&mut transaction);
        U256::ZERO.encode(&mut transaction);
        U256::ZERO.encode(&mut transaction);
        transaction.extend(rlp_list(&[]));

        let mut compact = match type_name {
            "Transaction" => vec![0x06, 0x06],
            "TransactionSigned" => vec![u8::MAX, 0x06, 0x06],
            _ => unreachable!(),
        };
        compact.extend(rlp_list(&transaction));
        compact
    }

    #[test]
    fn unrepresentable_frame_vectors_are_still_roundtripped() {
        for type_name in ["Transaction", "TransactionSigned"] {
            let vector = legacy_frame_vector(type_name, &[0xaa]);
            assert!(legacy_frame_vector_has_unrepresentable_fields(type_name, &vector).unwrap());
            assert_eq!(
                canonicalize_legacy_frame_vector_with_type(type_name, &vector).unwrap(),
                vector
            );

            let representable = legacy_frame_vector(type_name, &[0xaa; 20]);
            assert!(
                !legacy_frame_vector_has_unrepresentable_fields(type_name, &representable).unwrap()
            );
        }
    }
}

fn take_rlp_list<'a>(input: &mut &'a [u8]) -> Result<&'a [u8]> {
    let header = RlpHeader::decode(input)?;
    if !header.list {
        eyre::bail!("expected an RLP list in EIP-8141 compact vector")
    }
    let (payload, remaining) = input
        .split_at_checked(header.payload_length)
        .ok_or_else(|| eyre::eyre!("truncated RLP list in EIP-8141 compact vector"))?;
    *input = remaining;
    Ok(payload)
}

/// Returns the type name for the given type.
pub fn type_name<T>() -> String {
    // With alloy type transition <https://github.com/paradigmxyz/reth/pull/15768> the types are renamed, we map them here to the original name so that test vector files remain consistent
    let name = std::any::type_name::<T>();
    match name {
        "alloy_consensus::transaction::envelope::EthereumTypedTransaction<alloy_consensus::transaction::eip4844::TxEip4844>" => "Transaction".to_string(),
        "alloy_consensus::transaction::envelope::EthereumTxEnvelope<alloy_consensus::transaction::eip4844::TxEip4844>" => "TransactionSigned".to_string(),
        name => {
            name.split("::").last().unwrap_or(std::any::type_name::<T>()).to_string()
        }
    }
}
