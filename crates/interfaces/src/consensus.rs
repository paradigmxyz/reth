use async_trait::async_trait;
use reth_primitives::{
    BlockHash, BlockNumber, Header, InvalidTransactionError, SealedBlock, SealedHeader, B256, U256,
};
use std::fmt::Debug;

/// Re-export fork choice state
pub use reth_rpc_types::engine::ForkchoiceState;

/// Consensus is a protocol that chooses canonical chain.
#[async_trait]
#[auto_impl::auto_impl(&, Arc)]
pub trait Consensus: Debug + Send + Sync {
    /// Validate if header is correct and follows consensus specification.
    ///
    /// This is called on standalone header to check if all hashes are correct.
    fn validate_header(&self, header: &SealedHeader) -> Result<(), ConsensusError>;

    /// Validate that the header information regarding parent are correct.
    /// This checks the block number, timestamp, basefee and gas limit increment.
    ///
    /// This is called before properties that are not in the header itself (like total difficulty)
    /// have been computed.
    ///
    /// **This should not be called for the genesis block**.
    ///
    /// Note: Validating header against its parent does not include other Consensus validations.
    fn validate_header_against_parent(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError>;

    /// Validates the given headers
    ///
    /// This ensures that the first header is valid on its own and all subsequent headers are valid
    /// on its own and valid against its parent.
    ///
    /// Note: this expects that the headers are in natural order (ascending block number)
    fn validate_header_range(&self, headers: &[SealedHeader]) -> Result<(), ConsensusError> {
        if headers.is_empty() {
            return Ok(())
        }
        let first = headers.first().expect("checked empty");
        self.validate_header(first)?;
        let mut parent = first;
        for child in headers.iter().skip(1) {
            self.validate_header(child)?;
            self.validate_header_against_parent(child, parent)?;
            parent = child;
        }

        Ok(())
    }

    /// Validate if the header is correct and follows the consensus specification, including
    /// computed properties (like total difficulty).
    ///
    /// Some consensus engines may want to do additional checks here.
    ///
    /// Note: validating headers with TD does not include other Consensus validation.
    fn validate_header_with_total_difficulty(
        &self,
        header: &Header,
        total_difficulty: U256,
    ) -> Result<(), ConsensusError>;

    /// Validate a block disregarding world state, i.e. things that can be checked before sender
    /// recovery and execution.
    ///
    /// See the Yellow Paper sections 4.3.2 "Holistic Validity", 4.3.4 "Block Header Validity", and
    /// 11.1 "Ommer Validation".
    ///
    /// **This should not be called for the genesis block**.
    ///
    /// Note: validating blocks does not include other validations of the Consensus
    fn validate_block(&self, block: &SealedBlock) -> Result<(), ConsensusError>;
}

/// Consensus Errors
#[allow(missing_docs)]
#[derive(thiserror::Error, Debug, PartialEq, Eq, Clone)]
pub enum ConsensusError {
    /// Error when the gas used in the header exceeds the gas limit.
    #[error("Block used gas ({gas_used}) is greater than gas limit ({gas_limit}).")]
    HeaderGasUsedExceedsGasLimit {
        /// The gas used in the block header.
        gas_used: u64,
        /// The gas limit in the block header.
        gas_limit: u64,
    },

    /// Error when the hash of block ommer is different from the expected hash.
    #[error("Block ommer hash ({got:?}) is different from expected: ({expected:?})")]
    BodyOmmersHashDiff {
        /// The actual ommer hash.
        got: B256,
        /// The expected ommer hash.
        expected: B256,
    },

    /// Error when the state root in the block is different from the expected state root.
    #[error("Block state root ({got:?}) is different from expected: ({expected:?})")]
    BodyStateRootDiff {
        /// The actual state root.
        got: B256,
        /// The expected state root.
        expected: B256,
    },

    /// Error when the transaction root in the block is different from the expected transaction
    /// root.
    #[error("Block transaction root ({got:?}) is different from expected ({expected:?})")]
    BodyTransactionRootDiff {
        /// The actual transaction root.
        got: B256,
        /// The expected transaction root.
        expected: B256,
    },

    /// Error when the withdrawals root in the block is different from the expected withdrawals
    /// root.
    #[error("Block withdrawals root ({got:?}) is different from expected ({expected:?})")]
    BodyWithdrawalsRootDiff {
        /// The actual withdrawals root.
        got: B256,
        /// The expected withdrawals root.
        expected: B256,
    },

    /// Error when a block with a specific hash and number is already known.
    #[error("Block with [hash:{hash:?},number: {number}] is already known.")]
    BlockKnown {
        /// The hash of the known block.
        hash: BlockHash,
        /// The block number of the known block.
        number: BlockNumber,
    },

    /// Error when the parent hash of a block is not known.
    #[error("Block parent [hash:{hash:?}] is not known.")]
    ParentUnknown {
        /// The hash of the unknown parent block.
        hash: BlockHash,
    },

    /// Error when the block number does not match the parent block number.
    #[error(
        "Block number {block_number} does not match parent block number {parent_block_number}"
    )]
    ParentBlockNumberMismatch {
        /// The parent block number.
        parent_block_number: BlockNumber,
        /// The block number.
        block_number: BlockNumber,
    },

    /// Error when the parent hash does not match the expected parent hash.
    #[error(
        "Parent hash {got_parent_hash:?} does not match the expected {expected_parent_hash:?}"
    )]
    ParentHashMismatch {
        /// The expected parent hash.
        expected_parent_hash: B256,
        /// The actual parent hash.
        got_parent_hash: B256,
    },

    /// Error when the block timestamp is in the past compared to the parent timestamp.
    #[error("Block timestamp {timestamp} is in the past compared to the parent timestamp {parent_timestamp}.")]
    TimestampIsInPast {
        /// The parent block's timestamp.
        parent_timestamp: u64,
        /// The block's timestamp.
        timestamp: u64,
    },

    /// Error when the block timestamp is in the future compared to our clock time.
    #[error("Block timestamp {timestamp} is in the future compared to our clock time {present_timestamp}.")]
    TimestampIsInFuture {
        /// The block's timestamp.
        timestamp: u64,
        /// The current timestamp.
        present_timestamp: u64,
    },

    /// Error when the child gas limit exceeds the maximum allowed increase.
    #[error("Child gas_limit {child_gas_limit} max increase is {parent_gas_limit}/1024.")]
    GasLimitInvalidIncrease {
        /// The parent gas limit.
        parent_gas_limit: u64,
        /// The child gas limit.
        child_gas_limit: u64,
    },

    /// Error when the child gas limit exceeds the maximum allowed decrease.
    #[error("Child gas_limit {child_gas_limit} max decrease is {parent_gas_limit}/1024.")]
    GasLimitInvalidDecrease {
        /// The parent gas limit.
        parent_gas_limit: u64,
        /// The child gas limit.
        child_gas_limit: u64,
    },

    /// Error when the base fee is missing.
    #[error("Base fee missing.")]
    BaseFeeMissing,

    /// Error when the block's base fee is different from the expected base fee.
    #[error("Block base fee ({got}) is different than expected: ({expected}).")]
    BaseFeeDiff {
        /// The expected base fee.
        expected: u64,
        /// The actual base fee.
        got: u64,
    },

    /// Error when there is a transaction signer recovery error.
    #[error("Transaction signer recovery error.")]
    TransactionSignerRecoveryError,

    /// Error when the extra data length exceeds the maximum allowed.
    #[error("Extra data {len} exceeds max length.")]
    ExtraDataExceedsMax {
        /// The length of the extra data.
        len: usize,
    },

    /// Error when the difficulty after a merge is not zero.
    #[error("Difficulty after merge is not zero")]
    TheMergeDifficultyIsNotZero,

    /// Error when the nonce after a merge is not zero.
    #[error("Nonce after merge is not zero")]
    TheMergeNonceIsNotZero,

    /// Error when the ommer root after a merge is not empty.
    #[error("Ommer root after merge is not empty")]
    TheMergeOmmerRootIsNotEmpty,

    /// Error when the withdrawals root is missing.
    #[error("Missing withdrawals root")]
    WithdrawalsRootMissing,

    /// Error when an unexpected withdrawals root is encountered.
    #[error("Unexpected withdrawals root")]
    WithdrawalsRootUnexpected,

    /// Error when withdrawals are missing.
    #[error("Missing withdrawals")]
    BodyWithdrawalsMissing,

    /// Error when blob gas used is missing.
    #[error("Missing blob gas used")]
    BlobGasUsedMissing,

    /// Error when unexpected blob gas used is encountered.
    #[error("Unexpected blob gas used")]
    BlobGasUsedUnexpected,

    /// Error when excess blob gas is missing.
    #[error("Missing excess blob gas")]
    ExcessBlobGasMissing,

    /// Error when unexpected excess blob gas is encountered.
    #[error("Unexpected excess blob gas")]
    ExcessBlobGasUnexpected,

    /// Error when the parent beacon block root is missing.
    #[error("Missing parent beacon block root")]
    ParentBeaconBlockRootMissing,

    /// Error when an unexpected parent beacon block root is encountered.
    #[error("Unexpected parent beacon block root")]
    ParentBeaconBlockRootUnexpected,

    /// Error when blob gas used exceeds the maximum allowed.
    #[error("Blob gas used {blob_gas_used} exceeds maximum allowance {max_blob_gas_per_block}")]
    BlobGasUsedExceedsMaxBlobGasPerBlock {
        /// The actual blob gas used.
        blob_gas_used: u64,
        /// The maximum allowed blob gas per block.
        max_blob_gas_per_block: u64,
    },

    /// Error when blob gas used is not a multiple of blob gas per blob.
    #[error(
        "Blob gas used {blob_gas_used} is not a multiple of blob gas per blob {blob_gas_per_blob}"
    )]
    BlobGasUsedNotMultipleOfBlobGasPerBlob {
        /// The actual blob gas used.
        blob_gas_used: u64,
        /// The blob gas per blob.
        blob_gas_per_blob: u64,
    },

    /// Error when the blob gas used in the header does not match the expected blob gas used.
    #[error("Blob gas used in the header {header_blob_gas_used} does not match the expected blob gas used {expected_blob_gas_used}")]
    BlobGasUsedDiff {
        /// The blob gas used in the header.
        header_blob_gas_used: u64,
        /// The expected blob gas used.
        expected_blob_gas_used: u64,
    },

    /// Error when there is an invalid excess blob gas.
    #[error("Invalid excess blob gas. Expected: {expected}, got: {got}. Parent excess blob gas: {parent_excess_blob_gas}, parent blob gas used: {parent_blob_gas_used}.")]
    ExcessBlobGasDiff {
        /// The expected excess blob gas.
        expected: u64,
        /// The actual excess blob gas.
        got: u64,
        /// The parent excess blob gas.
        parent_excess_blob_gas: u64,
        /// The parent blob gas used.
        parent_blob_gas_used: u64,
    },

    /// Error for a transaction that violates consensus.
    #[error(transparent)]
    InvalidTransaction(#[from] InvalidTransactionError),
}
