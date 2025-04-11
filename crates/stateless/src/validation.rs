use std::{collections::HashMap, sync::Arc};

use crate::witness_db::WitnessDatabase;
use alloy_consensus::{Block, BlockHeader, Header};
use alloy_primitives::{keccak256, map::B256Map, B256};
use alloy_rlp::Decodable;
use alloy_rpc_types_debug::ExecutionWitness;
use reth_chainspec::ChainSpec;
use reth_ethereum_consensus::validate_block_post_execution;
use reth_evm::execute::{BlockExecutorProvider, Executor};
use reth_evm_ethereum::execute::EthExecutorProvider;
use reth_primitives::{RecoveredBlock, TransactionSigned};
use reth_revm::state::Bytecode;
use reth_trie::{iter::IntoParallelRefIterator, HashedPostState, KeccakKeyHasher};
use reth_trie_sparse::{blinded::DefaultBlindedProviderFactory, SparseStateTrie};

/// Errors that can occur during stateless validation.
#[derive(Debug, thiserror::Error)]
pub enum StatelessValidationError {
    /// Error when the number of ancestor headers exceeds the limit.
    #[error("ancestor header count ({count}) exceeds limit ({limit})")]
    AncestorHeaderLimitExceeded {
        /// The number of headers provided.
        count: usize,
        /// The limit.
        limit: usize,
    },

    /// Error when the ancestor headers do not form a contiguous chain.
    #[error("invalid ancestor chain")]
    InvalidAncestorChain,

    /// Error when revealing the witness data failed.
    #[error("failed to reveal witness data for pre-state root {pre_state_root}")]
    WitnessRevealFailed {
        /// The pre-state root used for verification.
        pre_state_root: B256,
    },

    /// Error during stateless block execution.
    #[error("stateless block execution failed")]
    StatelessExecutionFailed(String),

    /// Error during stateless post-execution validation.
    #[error("stateless post-execution validation failed: {0}")]
    // StatelessPostValidationFailed(#[from] ConsensusError),
    StatelessPostValidationFailed(String),

    /// Error during stateless state root calculation.
    #[error("stateless state root calculation failed")]
    StatelessStateRootCalculationFailed,

    /// Error calculating the pre-state root from the witness data.
    #[error("stateless pre-state root calculation failed")]
    StatelessPreStateRootCalculationFailed,

    /// Error when required ancestor headers are missing (e.g., parent header for pre-state root).
    #[error("missing required ancestor headers")]
    MissingAncestorHeader,

    /// Error when deserializing ancestor headers
    #[error("could not deserialize ancestor headers")]
    HeaderDeserializationFailed,

    /// Error when the computed state root does not match the one in the block header.
    #[error("mismatched post- state root: {got}\n {expected}")]
    PostStateRootMismatch {
        /// The computed post-state root
        got: B256,
        /// The expected post-state root; in the block header
        expected: B256,
    },

    /// Error when the computed pre-state root does not match the expected one.
    #[error("mismatched pre-state root: {got} \n {expected}")]
    PreStateRootMismatch {
        /// The computed pre-state root
        got: B256,
        /// The expected pre-state root from the previous block
        expected: B256,
    },
}

#[derive(Debug)]
/// TODO doc
pub struct Input {
    /// TODO doc
    pub current_block: RecoveredBlock<Block<TransactionSigned>>,
    /// TODO doc
    pub execution_witness: ExecutionWitness,
    /// TODO doc
    /// Note: // We need to use this Arc, because `EthEvmConfig` and a lot of internal
    /// reth components require it
    pub chain_spec: Arc<ChainSpec>,
}

/// Performs stateless validation of a block using the provided witness data.
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
///    limit using `compute_ancestor_hashes`. We must have at least one ancestor header, even if the
///    `BLOCKHASH` opcode is not used because we need the state root of the previous block to verify
///    the pre state reads.
///
/// 2. **Pre-State Verification:** Retrieves the expected `pre_state_root` from the parent header
///    from `ancestor_headers`. Verifies the provided [`ExecutionWitness`] against this root using
///    [`verify_execution_witness`].
///
/// 3. **Chain Verification:** The code currently does not verify the [`ChainSpec`] and expects a
///    higher level function to assert that this is correct by, for example, asserting that it is
///    equal to the Ethereum Mainnet `ChainSpec` or asserting against the genesis hash that this
///    `ChainSpec` defines.
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
pub fn stateless_validation(
    current_block: RecoveredBlock<Block<TransactionSigned>>,
    witness: ExecutionWitness,
    chain_spec: Arc<ChainSpec>,
) -> Result<B256, StatelessValidationError> {
    // TODO: clean this up, the clone and vector allocation is undesirable
    let _ancestor_headers: Result<Vec<_>, _> = witness
        .headers
        .iter()
        .map(|serialized_header| {
            let header: Vec<u8> = serialized_header.clone().into();
            Header::decode(&mut &header[..])
        })
        .collect();
    let ancestor_headers =
        _ancestor_headers.map_err(|_| StatelessValidationError::HeaderDeserializationFailed)?;

    // Check that the ancestor headers form a contiguous chain and are not just random headers.
    let ancestor_hashes = compute_ancestor_hashes(&current_block, &ancestor_headers)?;

    // Get the last ancestor header and retrieve its state root.
    //
    // There should be at least one ancestor header, this is because we need the parent header to
    // retrieve the previous state root.
    // The edge case here would be the genesis block, but we do not create proofs for the genesis
    // block.
    let pre_state_root = match ancestor_headers.last() {
        Some(prev_header) => prev_header.state_root,
        None => return Err(StatelessValidationError::MissingAncestorHeader),
    };

    // First verify that the pre-state reads are correct
    let (mut sparse_trie, bytecode) = verify_execution_witness(&witness, pre_state_root)?;

    // Create an in-memory database that will use the reads to validate the block
    let db = WitnessDatabase::new(&sparse_trie, bytecode, ancestor_hashes);

    // Execute the block
    let basic_block_executor = EthExecutorProvider::ethereum(chain_spec.clone());
    let executor = basic_block_executor.executor(db);
    let output = executor
        .execute(&current_block)
        .map_err(|e| StatelessValidationError::StatelessExecutionFailed(e.to_string()))?;

    // Post validation checks
    validate_block_post_execution(&current_block, &chain_spec, &output.receipts, &output.requests)
        .map_err(|err| StatelessValidationError::StatelessPostValidationFailed(err.to_string()))?;

    // Compute and check the post state root
    // TODO: Remove rayon
    let hashed_state =
        HashedPostState::from_bundle_state::<KeccakKeyHasher>(output.state.state.par_iter());
    let state_root = crate::root::calculate_state_root(&mut sparse_trie, hashed_state)
        .map_err(|_e| StatelessValidationError::StatelessStateRootCalculationFailed)?;
    if state_root != current_block.state_root {
        return Err(StatelessValidationError::PostStateRootMismatch {
            got: state_root,
            expected: current_block.state_root,
        });
    }

    // Return block hash
    Ok(current_block.hash_slow())
}

/// Verifies execution witness [`ExecutionWitness`] against an expected pre-state root.
///
/// This function takes the RLP-encoded values provided in [`ExecutionWitness`]
/// (which includes state trie nodes, storage trie nodes, and contract bytecode)
/// and uses it to populate a new [`SparseStateTrie`].
///
/// If the computed root hash matches the `pre_state_root`, it signifies that the
/// provided execution witness is consistent with that pre-state root. In this case, the function
/// returns the populated [`SparseStateTrie`] and a [`B256Map`] containing the
/// contract bytecode (mapping code hash to [`Bytecode`]).
///
/// The bytecode has a separate mapping because the [`SparseStateTrie`] does not store the
/// contract bytecode, only the hash of it (code hash).
///
/// If the roots do not match, it returns `None`, indicating the witness is invalid
/// for the given `pre_state_root`.
// Note: This approach might be inefficient for ZKVMs requiring minimal memory operations, which
// would explain why they have for the most part re-implemented this function.
pub fn verify_execution_witness(
    witness: &ExecutionWitness,
    pre_state_root: B256,
) -> Result<(SparseStateTrie, B256Map<Bytecode>), StatelessValidationError> {
    let mut trie = SparseStateTrie::new(DefaultBlindedProviderFactory);
    let mut state_witness = B256Map::default();
    let mut bytecode = B256Map::default();

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
    trie.reveal_witness(pre_state_root, &state_witness)
        .map_err(|_e| StatelessValidationError::WitnessRevealFailed { pre_state_root })?;

    // Calculate the root
    let computed_root = trie
        .root()
        .map_err(|_e| StatelessValidationError::StatelessPreStateRootCalculationFailed)?;

    if computed_root == pre_state_root {
        Ok((trie, bytecode))
    } else {
        Err(StatelessValidationError::PreStateRootMismatch {
            got: computed_root,
            expected: pre_state_root,
        })
    }
}

/// `BLOCKHASH_HISTORICAL_HASH_LIMIT` specifies the maximum number of historical
/// block hashes that the [BLOCKHASH](https://www.evm.codes/?fork=cancun#40)
/// opcode is specified to allow.
const BLOCKHASH_HISTORICAL_HASH_LIMIT: usize = 256;

/// Verifies the contiguity, number of ancestor headers and extracts their hashes.
///
/// This function is used to prepare the data required for the `BLOCKHASH`
/// opcode in a stateless execution context.
///
/// It performs two main checks:
///
/// 1. Ensures that the number of provided `ancestor_headers` does not exceed the
///    `BLOCKHASH_HISTORICAL_HASH_LIMIT` (256). This limit is defined by the `BLOCKHASH` opcode and
///    has nothing to do with stateless.
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
) -> Result<HashMap<u64, B256>, StatelessValidationError> {
    // Check that we have `BLOCKHASH_HISTORICAL_HASH_LIMIT` number of
    // ancestors.
    if ancestor_headers.len() >= BLOCKHASH_HISTORICAL_HASH_LIMIT {
        return Err(StatelessValidationError::AncestorHeaderLimitExceeded {
            count: ancestor_headers.len(),
            limit: BLOCKHASH_HISTORICAL_HASH_LIMIT,
        });
    }
    let mut ancestor_hashes = HashMap::with_capacity(ancestor_headers.len());

    let mut child_header = current_block.header();

    // Next verify that headers supplied are contiguous
    for parent_header in ancestor_headers.iter().rev() {
        let parent_hash = child_header.parent_hash();
        ancestor_hashes.insert(parent_header.number, parent_hash);

        if parent_hash != parent_header.hash_slow() {
            return Err(StatelessValidationError::InvalidAncestorChain); // Blocks must be contiguous
        }

        if parent_header.number + 1 != child_header.number {
            return Err(StatelessValidationError::InvalidAncestorChain); // Header number should be
                                                                        // contiguous
        }

        child_header = parent_header
    }

    Ok(ancestor_hashes)
}
