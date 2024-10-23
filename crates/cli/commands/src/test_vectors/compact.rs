use alloy_primitives::{hex, private::getrandom::getrandom, TxKind};
use arbitrary::Arbitrary;
use eyre::Result;
use proptest::{
    prelude::{ProptestConfig, RngCore},
    test_runner::{TestRng, TestRunner},
};
use reth_codecs::alloy::{
    authorization_list::Authorization,
    genesis_account::GenesisAccount,
    header::{Header, HeaderExt},
    transaction::{eip2930::TxEip2930, eip1559::TxEip1559, eip4844::TxEip4844, eip7702::TxEip7702, legacy::TxLegacy},
    withdrawal::Withdrawal,
};
use reth_db::{
    models::{AccountBeforeTx, StoredBlockBodyIndices, StoredBlockOmmers, StoredBlockWithdrawals},
    ClientVersion,
};
use reth_fs_util as fs;
use reth_primitives::{
    Account, Log, LogData, Receipt, StorageEntry, Transaction, TransactionSignedNoHash, TxType
};
use reth_prune_types::{PruneCheckpoint, PruneMode};
use reth_stages_types::{
    AccountHashingCheckpoint, CheckpointBlockRange, EntitiesCheckpoint, ExecutionCheckpoint,
    HeadersCheckpoint, IndexHistoryCheckpoint, StageCheckpoint, StageUnitCheckpoint,
    StorageHashingCheckpoint,
};
use reth_trie::{hash_builder::HashBuilderValue, TrieMask};
use reth_trie_common::{hash_builder::HashBuilderState, StoredNibbles, StoredNibblesSubKey};
use std::{collections::HashSet, fs::File, io::BufReader, sync::LazyLock};

const VECTORS_FOLDER: &str = "testdata/micro/compact";
const VECTOR_SIZE: usize = 100;

macro_rules! compact_types {
    (regular: [$($regular_ty:ident),*], identifier: [$($id_ty:ident),*]) => {
        const GENERATE_VECTORS: &[fn(&mut TestRunner) -> Result<()>] = &[
            $(
                generate_vector::<$regular_ty> as fn(&mut TestRunner) -> Result<()>,
            )*
            $(
                generate_vector::<$id_ty> as fn(&mut TestRunner) -> Result<()>,
            )*
        ];

        const READ_VECTORS: &[fn() -> Result<()>] = &[
            $(
                read_vector::<$regular_ty> as fn() -> Result<()>,
            )*
            $(
                read_vector::<$id_ty> as fn() -> Result<()>,
            )*
        ];

        static IDENTIFIER_TYPE: LazyLock<HashSet<String>> = LazyLock::new(|| {
            let mut map = HashSet::new();
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
        Receipt,
        // reth_codecs::alloy
        Authorization,
        GenesisAccount,
        Header,
        HeaderExt,
        Withdrawal,
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
        // TxDeposit, TODO(joshie): optimism
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
        // Manual implementations
        TransactionSignedNoHash,
        // Bytecode, // todo revm arbitrary
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
        // Signature todo we for v we only store parity(true || false), while v can take more values
        Transaction,
        TxType,
        TxKind
    ]
);

pub(crate) fn generate_vectors() -> Result<()> {
    // Prepare random seed for test (same method as used by proptest)
    let mut seed = [0u8; 32];
    getrandom(&mut seed)?;
    println!("Seed for compact test vectors: {:?}", hex::encode_prefixed(seed));

    // Start the runner with the seed
    let config = ProptestConfig::default();
    let rng = TestRng::from_seed(config.rng_algorithm, &seed);
    let mut runner = TestRunner::new_with_rng(config, rng);

    fs::create_dir_all(VECTORS_FOLDER)?;

    for generate_fn in GENERATE_VECTORS {
        generate_fn(&mut runner)?;
    }

    Ok(())
}

pub fn read_vectors() -> Result<()> {
    fs::create_dir_all(VECTORS_FOLDER)?;

    for read_fn in READ_VECTORS {
        read_fn()?;
    }

    Ok(())
}

/// Generates test vectors for a specific type `T`
fn generate_vector<T>(runner: &mut TestRunner) -> Result<()>
where
    T: for<'a> Arbitrary<'a>
        + reth_codecs::Compact
        + serde::Serialize
        + Clone
        + std::fmt::Debug
        + 'static,
{
    let type_name = type_name::<T>();
    let mut bytes = std::iter::repeat(0u8).take(256).collect::<Vec<u8>>();
    let mut compact_buffer = vec![];

    let mut values = Vec::with_capacity(VECTOR_SIZE);
    for _ in 0..VECTOR_SIZE {
        runner.rng().fill_bytes(&mut bytes);
        compact_buffer.clear();

        let obj = T::arbitrary(&mut arbitrary::Unstructured::new(&bytes))?;
        let res = obj.to_compact(&mut compact_buffer);

        if IDENTIFIER_TYPE.contains(&type_name) {
            compact_buffer.push(res as u8);
        }

        values.push((obj, hex::encode(&compact_buffer)));
    }

    serde_json::to_writer(
        std::io::BufWriter::new(
            std::fs::File::create(format!("{VECTORS_FOLDER}/{}.json", &type_name)).unwrap(),
        ),
        &values,
    )?;

    println!("{} ✅", &type_name);

    Ok(())
}

/// Reads vectors from the file and compares the original T with the one reconstructed using
/// T::from_compact.
fn read_vector<T>() -> Result<()>
where
    T: serde::de::DeserializeOwned + reth_codecs::Compact + PartialEq + Clone + std::fmt::Debug,
{
    let type_name = type_name::<T>();

    // Read the file where the vectors are stored
    let file_path = format!("{VECTORS_FOLDER}/{}.json", &type_name);
    let file = File::open(&file_path)?;
    let reader = BufReader::new(file);

    let stored_values: Vec<(T, String)> = serde_json::from_reader(reader)?;
    let mut buffer = vec![];

    for (original, hex_str) in stored_values {
        let mut compact_bytes = hex::decode(hex_str)?;
        let mut identifier = None;

        if IDENTIFIER_TYPE.contains(&type_name) {
            identifier = compact_bytes.pop().map(|b| b as usize);
        }

        let len_or_identifier = identifier.unwrap_or(compact_bytes.len());
        let (reconstructed, _) = T::from_compact(&compact_bytes, len_or_identifier);

        if original != reconstructed {
            println!("{} ❌", &type_name);
            panic!("mismatch found on {original:?} and {reconstructed:?}");
        }

        // Sanity check to ensure we encode the same way
        buffer.clear();
        reconstructed.to_compact(&mut buffer);
        assert_eq!(buffer, compact_bytes);
    }

    println!("{} ✅", &type_name);

    Ok(())
}

fn type_name<T>() -> String {
    std::any::type_name::<T>().replace("::", "__")
}
