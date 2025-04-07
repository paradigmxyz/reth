use std::{collections::HashMap, sync::Arc};

use crate::witness_db::WitnessDatabase;
use alloy_consensus::{Block, BlockHeader, Header};
use alloy_primitives::{keccak256, map::B256Map, B256};
use alloy_rpc_types_debug::ExecutionWitness;
use reth_chainspec::ChainSpec;
use reth_ethereum_consensus::validate_block_post_execution;
use reth_evm::execute::{BlockExecutorProvider, Executor};
use reth_evm_ethereum::execute::EthExecutorProvider;
use reth_primitives::{RecoveredBlock, TransactionSigned};
use reth_revm::state::Bytecode;
use reth_trie::{iter::IntoParallelRefIterator, HashedPostState, KeccakKeyHasher};
use reth_trie_sparse::{blinded::DefaultBlindedProviderFactory, SparseStateTrie};

// TODO: In some places, I call witness data ExecutionWitness and in other places, I refer to it as
// TODO: the set of all inputs including ancestor headers.  witness data should only refer to the
// TODO: whole set.

/// Performs stateless validation of a block using provided witness data.
///
/// This function attempts to fully validate a given `current_block` statelessly, ie without access
/// to a persistent database.
/// It relies entirely on the `witness` data and `ancestor_headers`
/// provided alongside the block.
///
/// The witness data is validated in the following way:
///
/// 1. **Ancestor Header Verification:** Checks if the `ancestor_headers` are present, form a
///    contiguous chain back from `current_block`'s parent, and do not exceed the `BLOCKHASH` opcode
///    limit using [`compute_ancestor_hashes`]. We must have at least one ancestor header, even if
///    the `BLOCKHASH` opcode is not used because we need the state root of the previous block to
///    verify the pre state reads.
///
/// 2. **Pre-State Verification:** Retrieves the expected `pre_state_root` from the parent header
///    from `ancestor_headers`. Verifies the provided [`ExecutionWitness`] against this root using
///    [`verify_execution_witness`].
///
/// 3. **Chain Verification:** The code currently does not verify the [`ChainSpec`] and expects a
///    higher level function to assert that this is correct by, for example, asserting that it is
///    equal to the Ethereum Mainnet Chainspec or asserting against the genesis hash that this
///    ChainSpec defines.
///
/// High Level Overview of functionality:
///
/// - Verify all state accesses against a trusted pre-state root
/// - Put all state accesses into an in-memory database
/// - Use the in-memory database to execute the block
/// - Validate the output of block execution (e.g. receipts, logs, requests)
/// - Compute the post-state root using the state-diff from block execution
/// - Check that the post-state root is the state root in the block.
///
/// If all steps succeed the function returns `Some` containing the hash of the validated
/// `current_block`.
// TODO(Note): The code currently unwraps in a lot of places. This will be cleaned up.
pub fn stateless_validation(
    current_block: RecoveredBlock<Block<TransactionSigned>>,
    witness: ExecutionWitness,
    ancestor_headers: Vec<Header>,
    chain_spec: Arc<ChainSpec>,
) -> Option<B256> {
    // There should be at least one ancestor header, this is because we need the parent header to
    // retrieve the previous state root.
    // The edge case here would be the genesis block, but we do not create proofs for the genesis
    // block.
    if ancestor_headers.is_empty() {
        return None;
    }

    // 0. Check that the ancestor headers form a contiguous chain and are not just random headers.
    //
    // This would make BLOCKHASH unsafe.
    let ancestor_hashes = compute_ancestor_hashes(&current_block, &ancestor_headers).unwrap();

    // Get the last ancestor header and retrieve its state root.
    // TODO: replace expect with a match and remove the first .is_empty call
    let pre_state_root =
        ancestor_headers.last().expect("There should be atleast one ancestor header").state_root;

    // 1. First verify that the pre-state reads are correct
    let (mut sparse_trie, bytecode) = verify_execution_witness(&witness, pre_state_root)?;

    // 2. Create an in-memory database that will use the reads to validate the block
    let db = WitnessDatabase::new(&sparse_trie, bytecode, ancestor_hashes);

    // 3. Execute the block
    let basic_block_executor = EthExecutorProvider::ethereum(chain_spec.clone());

    let executor = basic_block_executor.executor(db);
    let output = executor.execute(&current_block).unwrap();

    // 4. Post validation checks
    validate_block_post_execution(&current_block, &chain_spec, &output.receipts, &output.requests)
        .unwrap();

    // 5. Compute and check the post state root
    // TODO: Remove rayon
    let hashed_state =
        HashedPostState::from_bundle_state::<KeccakKeyHasher>(output.state.state.par_iter());
    let state_root = crate::root::calculate_state_root(&mut sparse_trie, hashed_state).unwrap();
    if state_root != current_block.state_root {
        return None;
    }

    // 6. return block hash
    return Some(current_block.hash_slow());
}

/// Verifies execution witness data [`ExecutionWitness`] against an expected pre-state root.
///
/// This function takes the RLP-encoded witness data provided in [`ExecutionWitness`]
/// (which includes state trie nodes, storage trie nodes, and contract bytecode)
/// and uses it to populate a new [`SparseStateTrie`].
///
/// If the computed root hash matches the `pre_state_root`, it signifies that the
/// provided witness data is consistent with that state root. In this case, the function
/// returns the populated [`SparseStateTrie`] and a [`B256Map`] containing the
/// contract bytecode (mapping code hash to [`Bytecode`]).
///
/// The bytecode has a separate mapping because the [`SparseStateTrie`] does not store the
/// contract bytecode, only the hash of it.
///
/// If the roots do not match, it returns `None`, indicating the witness is invalid
/// for the given `pre_state_root`.
///
/// # Parameters
/// - `witness`: The [`ExecutionWitness`] containing RLP-encoded trie nodes and bytecode necessary
///   to prove the pre-state against the `pre_state_root`.
/// - `pre_state_root`: The expected [`B256`] state root hash before execution.
///
/// # Returns
/// - `Some((SparseStateTrie, B256Map<Bytecode>))` if the witness is valid for the `pre_state_root`.
/// - `None` if the computed root does not match the `pre_state_root`.
// Note: This approach might be inefficient for ZKVMs requiring minimal memory operations, which
// would explain why they have for the most part re-implemented this function.

pub fn verify_execution_witness(
    // Witness for the pre_state reads
    witness: &ExecutionWitness,
    pre_state_root: B256,
) -> Option<(SparseStateTrie, B256Map<Bytecode>)> {
    let mut trie = SparseStateTrie::new(DefaultBlindedProviderFactory);
    let mut state_witness = B256Map::default();
    let mut bytecode = B256Map::default();

    // Add all witness components to our map
    for rlp_encoded in &witness.state {
        let hash = keccak256(rlp_encoded);
        state_witness.insert(hash, rlp_encoded.clone());
    }
    for rlp_encoded in &witness.keys {
        let hash = keccak256(rlp_encoded);
        state_witness.insert(hash, rlp_encoded.clone());
    }
    for rlp_encoded in &witness.codes {
        let hash = keccak256(rlp_encoded);
        state_witness.insert(hash, rlp_encoded.clone());
        bytecode.insert(hash, Bytecode::new_raw(rlp_encoded.clone()));
    }

    // Reveal the witness with our state root
    trie.reveal_witness(pre_state_root, &state_witness).unwrap();

    // Calculate the root
    let computed_root = trie.root().unwrap();

    (computed_root == pre_state_root).then(|| (trie, bytecode))
}

/// BLOCKHASH_HISTORICAL_HASH_LIMIT specifies the maximum number of historical
/// block hashes that the BLOCKHASH opcode is specified to allow.
const BLOCKHASH_HISTORICAL_HASH_LIMIT: usize = 256;

/// Verifies the contiguity, number of ancestor headers and extracts their hashes.
///
/// This function is used to prepare the data required for the `BLOCKHASH`
/// opcode in a stateless execution context.
///
/// It performs two main checks:
///
/// 1. Ensures that the number of provided `ancestor_headers` plus the `current_block` itself does
///    not exceed the `BLOCKHASH_HISTORICAL_HASH_LIMIT` (256). This limit is defined by the
///    `BLOCKHASH` opcode and has nothing to do with stateless.
/// 2. Verifies that the provided `ancestor_headers` form a valid, unbroken chain leading back from
///    the parent of the `current_block`.
///
/// Note: This function becomes obsolete if EIP-2935 is implemented.
///
/// If both checks pass, it returns a [`HashMap`] mapping the block number of each
/// ancestor header to its corresponding block hash.
fn compute_ancestor_hashes(
    current_block: &RecoveredBlock<Block<TransactionSigned>>,
    ancestor_headers: &[Header],
) -> Option<HashMap<u64, B256>> {
    // We should only have at most BLOCKHASH_OPCODE_LIMIT -1 number of ancestors
    // because we include the current block in the blockhash limit
    if ancestor_headers.len() > BLOCKHASH_HISTORICAL_HASH_LIMIT {
        return None;
    }
    let mut ancestor_hashes = HashMap::with_capacity(ancestor_headers.len());

    let mut child_header = current_block.header();

    // Next verify that headers supplied are contiguous
    for parent_header in ancestor_headers.iter().rev() {
        let parent_hash = child_header.parent_hash();
        ancestor_hashes.insert(parent_header.number, parent_hash);

        if parent_hash != parent_header.hash_slow() {
            return None; // Blocks must be contiguous
        }

        // TODO: Check the parent block number too?

        child_header = parent_header
    }

    Some(ancestor_hashes)
}
