use crate::{trie::StatelessTrie, witness_db::WitnessDatabase, ExecutionWitness};
use alloc::{
    boxed::Box,
    collections::BTreeMap,
    fmt::Debug,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use alloy_consensus::{BlockHeader, Header};
use alloy_primitives::B256;
use alloy_rlp::Decodable;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_consensus::{Consensus, HeaderValidator};
use reth_errors::ConsensusError;
use reth_ethereum_consensus::{validate_block_post_execution, EthBeaconConsensus};
use reth_ethereum_primitives::{Block, EthPrimitives};
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_primitives_traits::{block::error::BlockRecoveryError, Block as _, RecoveredBlock};
use reth_trie_common::{HashedPostState, KeccakKeyHasher};

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

    /// Error during consensus validation of the block.
    #[error("consensus validation failed: {0}")]
    ConsensusValidationFailed(#[from] ConsensusError),

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

    /// Error when recovering signers
    #[error("error recovering the signers in the block")]
    SignerRecovery(#[from] Box<BlockRecoveryError<Block>>),
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
///    from `ancestor_headers`. Verifies the provided [`ExecutionWitness`] against the
///    `pre_state_root`.
///
/// 3. **Chain Verification:** The code currently does not verify the [`EthChainSpec`] and expects a
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
pub fn stateless_validation<ChainSpec, E>(
    current_block: Block,
    witness: ExecutionWitness,
    chain_spec: Arc<ChainSpec>,
    evm_config: E,
) -> Result<B256, StatelessValidationError>
where
    ChainSpec: Send + Sync + EthChainSpec + EthereumHardforks + Debug,
    E: ConfigureEvm<Primitives = EthPrimitives> + Clone + 'static,
{
    let current_block = current_block
        .try_into_recovered()
        .map_err(|err| StatelessValidationError::SignerRecovery(Box::new(err)))?;

    let mut ancestor_headers: Vec<Header> = witness
        .headers
        .iter()
        .map(|serialized_header| {
            let bytes = serialized_header.as_ref();
            Header::decode(&mut &bytes[..])
                .map_err(|_| StatelessValidationError::HeaderDeserializationFailed)
        })
        .collect::<Result<_, _>>()?;
    // Sort the headers by their block number to ensure that they are in
    // ascending order.
    ancestor_headers.sort_by_key(|header| header.number());

    // Validate block against pre-execution consensus rules
    validate_block_consensus(chain_spec.clone(), &current_block)?;

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
    let (mut trie, bytecode) = StatelessTrie::new(&witness, pre_state_root)?;

    // Create an in-memory database that will use the reads to validate the block
    let db = WitnessDatabase::new(&trie, bytecode, ancestor_hashes);

    // Execute the block
    let executor = evm_config.executor(db);
    let output = executor
        .execute(&current_block)
        .map_err(|e| StatelessValidationError::StatelessExecutionFailed(e.to_string()))?;

    // Post validation checks
    validate_block_post_execution(&current_block, &chain_spec, &output.receipts, &output.requests)
        .map_err(StatelessValidationError::ConsensusValidationFailed)?;

    // Compute and check the post state root
    let hashed_state = HashedPostState::from_bundle_state::<KeccakKeyHasher>(&output.state.state);
    let state_root = trie.calculate_state_root(hashed_state)?;
    if state_root != current_block.state_root {
        return Err(StatelessValidationError::PostStateRootMismatch {
            got: state_root,
            expected: current_block.state_root,
        });
    }

    // Return block hash
    Ok(current_block.hash_slow())
}

/// Performs consensus validation checks on a block without execution or state validation.
///
/// This function validates a block against Ethereum consensus rules by:
///
/// 1. **Difficulty Validation:** Validates the header with total difficulty to verify proof-of-work
///    (pre-merge) or to enforce post-merge requirements.
///
/// 2. **Header Validation:** Validates the sealed header against protocol specifications,
///    including:
///    - Gas limit checks
///    - Base fee validation for EIP-1559
///    - Withdrawals root validation for Shanghai fork
///    - Blob-related fields validation for Cancun fork
///
/// 3. **Pre-Execution Validation:** Validates block structure, transaction format, signature
///    validity, and other pre-execution requirements.
///
/// This function acts as a preliminary validation before executing and validating the state
/// transition function.
fn validate_block_consensus<ChainSpec>(
    chain_spec: Arc<ChainSpec>,
    block: &RecoveredBlock<Block>,
) -> Result<(), StatelessValidationError>
where
    ChainSpec: Send + Sync + EthChainSpec + EthereumHardforks + Debug,
{
    let consensus = EthBeaconConsensus::new(chain_spec);

    consensus.validate_header(block.sealed_header())?;

    consensus.validate_block_pre_execution(block)?;

    Ok(())
}

/// Verifies the contiguity, number of ancestor headers and extracts their hashes.
///
/// This function is used to prepare the data required for the `BLOCKHASH`
/// opcode in a stateless execution context.
///
/// It verifies that the provided `ancestor_headers` form a valid, unbroken chain leading back from
///    the parent of the `current_block`.
///
/// Note: This function becomes obsolete if EIP-2935 is implemented.
/// Note: The headers are assumed to be in ascending order.
///
/// If both checks pass, it returns a [`BTreeMap`] mapping the block number of each
/// ancestor header to its corresponding block hash.
fn compute_ancestor_hashes(
    current_block: &RecoveredBlock<Block>,
    ancestor_headers: &[Header],
) -> Result<BTreeMap<u64, B256>, StatelessValidationError> {
    let mut ancestor_hashes = BTreeMap::new();

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
