use async_trait::async_trait;
use reth_primitives::{
    BlockHash, BlockNumber, Header, InvalidTransactionError, SealedBlock, SealedHeader, H256, U256,
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
    /// This is called on standalone header to check if all hashe
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
    #[error("Block used gas ({gas_used:?}) is greater than gas limit ({gas_limit:?}).")]
    HeaderGasUsedExceedsGasLimit { gas_used: u64, gas_limit: u64 },
    #[error("Block ommer hash ({got:?}) is different from expected: ({expected:?})")]
    BodyOmmersHashDiff { got: H256, expected: H256 },
    #[error("Block state root ({got:?}) is different from expected: ({expected:?})")]
    BodyStateRootDiff { got: H256, expected: H256 },
    #[error("Block transaction root ({got:?}) is different from expected ({expected:?})")]
    BodyTransactionRootDiff { got: H256, expected: H256 },
    #[error("Block receipts root ({got:?}) is different from expected ({expected:?})")]
    BodyReceiptsRootDiff { got: H256, expected: H256 },
    #[error("Block withdrawals root ({got:?}) is different from expected ({expected:?})")]
    BodyWithdrawalsRootDiff { got: H256, expected: H256 },
    #[error("Block with [hash:{hash:?},number: {number:}] is already known.")]
    BlockKnown { hash: BlockHash, number: BlockNumber },
    #[error("Block parent [hash:{hash:?}] is not known.")]
    ParentUnknown { hash: BlockHash },
    #[error("Block number {block_number:?} is mismatch with parent block number {parent_block_number:?}")]
    ParentBlockNumberMismatch { parent_block_number: BlockNumber, block_number: BlockNumber },
    #[error(
    "Block timestamp {timestamp:?} is in past in comparison with parent timestamp {parent_timestamp:?}."
    )]
    TimestampIsInPast { parent_timestamp: u64, timestamp: u64 },
    #[error("Block timestamp {timestamp:?} is in future in comparison of our clock time {present_timestamp:?}.")]
    TimestampIsInFuture { timestamp: u64, present_timestamp: u64 },
    #[error("Child gas_limit {child_gas_limit:?} max increase is {parent_gas_limit:?}/1024.")]
    GasLimitInvalidIncrease { parent_gas_limit: u64, child_gas_limit: u64 },
    #[error("Child gas_limit {child_gas_limit:?} max decrease is {parent_gas_limit:?}/1024.")]
    GasLimitInvalidDecrease { parent_gas_limit: u64, child_gas_limit: u64 },
    #[error("Base fee missing.")]
    BaseFeeMissing,
    #[error("Block base fee ({got:?}) is different then expected: ({expected:?}).")]
    BaseFeeDiff { expected: u64, got: u64 },
    #[error("Transaction signer recovery error.")]
    TransactionSignerRecoveryError,
    #[error(
        "Transaction count {transaction_count} is different from receipt count {receipt_count}"
    )]
    TransactionReceiptCountDiff { transaction_count: usize, receipt_count: usize },
    #[error("Transaction had receipt of different type")]
    TransactionTypeReceiptTypeDiff,
    #[error("Extra data {len} exceeds max length: ")]
    ExtraDataExceedsMax { len: usize },
    #[error("Difficulty after merge is not zero")]
    TheMergeDifficultyIsNotZero,
    #[error("Nonce after merge is not zero")]
    TheMergeNonceIsNotZero,
    #[error("Ommer root after merge is not empty")]
    TheMergeOmmerRootIsNotEmpty,
    #[error("Mix hash after merge is not zero")]
    TheMergeMixHashIsNotZero,
    #[error("Missing withdrawals root")]
    WithdrawalsRootMissing,
    #[error("Unexpected withdrawals root")]
    WithdrawalsRootUnexpected,
    #[error("Withdrawal index #{got} is invalid. Expected: #{expected}.")]
    WithdrawalIndexInvalid { got: u64, expected: u64 },
    #[error("Missing withdrawals")]
    BodyWithdrawalsMissing,
    /// Error for a transaction that violates consensus.
    #[error(transparent)]
    InvalidTransaction(#[from] InvalidTransactionError),
}
