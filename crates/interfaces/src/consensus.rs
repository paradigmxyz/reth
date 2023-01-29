use async_trait::async_trait;
use reth_primitives::{BlockHash, BlockNumber, SealedBlock, SealedHeader, H256};
use std::fmt::Debug;
use tokio::sync::watch::{error::SendError, Receiver};

/// Re-export fork choice state
pub use reth_rpc_types::engine::ForkchoiceState;

/// Consensus is a protocol that chooses canonical chain.
#[async_trait]
#[auto_impl::auto_impl(&, Arc)]
pub trait Consensus: Debug + Send + Sync {
    /// Get a receiver for the fork choice state
    fn fork_choice_state(&self) -> Receiver<ForkchoiceState>;

    /// Notifies all listeners of the latest [ForkchoiceState].
    fn notify_fork_choice_state(
        &self,
        state: ForkchoiceState,
    ) -> Result<(), SendError<ForkchoiceState>>;

    /// Validate if header is correct and follows consensus specification.
    ///
    /// **This should not be called for the genesis block**.
    fn validate_header(&self, header: &SealedHeader, parent: &SealedHeader) -> Result<(), Error>;

    /// Validate a block disregarding world state, i.e. things that can be checked before sender
    /// recovery and execution.
    ///
    /// See the Yellow Paper sections 4.3.2 "Holistic Validity", 4.3.4 "Block Header Validity", and
    /// 11.1 "Ommer Validation".
    ///
    /// **This should not be called for the genesis block**.
    fn pre_validate_block(&self, block: &SealedBlock) -> Result<(), Error>;

    /// After the Merge (aka Paris) block rewards became obsolete.
    ///
    /// This flag is needed as reth's changeset is indexed on transaction level granularity.
    ///
    /// More info [here](https://github.com/paradigmxyz/reth/issues/237)
    fn has_block_reward(&self, block_num: BlockNumber) -> bool;
}

/// Consensus Errors
#[allow(missing_docs)]
#[derive(thiserror::Error, Debug, PartialEq, Eq, Clone)]
pub enum Error {
    #[error("Block used gas ({gas_used:?}) is greater than gas limit ({gas_limit:?}).")]
    HeaderGasUsedExceedsGasLimit { gas_used: u64, gas_limit: u64 },
    #[error("Block ommer hash ({got:?}) is different then expected: ({expected:?})")]
    BodyOmmersHashDiff { got: H256, expected: H256 },
    #[error("Block transaction root ({got:?}) is different then expected: ({expected:?})")]
    BodyTransactionRootDiff { got: H256, expected: H256 },
    #[error("Block receipts root ({got:?}) is different then expected: ({expected:?}).")]
    BodyReceiptsRootDiff { got: H256, expected: H256 },
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
    #[error("Transaction eip1559 priority fee is more then max fee.")]
    TransactionPriorityFeeMoreThenMaxFee,
    #[error("Transaction chain_id does not match.")]
    TransactionChainId,
    #[error("Transaction max fee is less them block base fee.")]
    TransactionMaxFeeLessThenBaseFee,
    #[error("Transaction signer does not have account.")]
    SignerAccountNotExisting,
    #[error("Transaction signer has bytecode set.")]
    SignerAccountHasBytecode,
    #[error("Transaction nonce is not consistent.")]
    TransactionNonceNotConsistent,
    #[error("Account does not have enough funds ({available_funds:?}) to cover transaction max fee: {max_fee:?}.")]
    InsufficientFunds { max_fee: u128, available_funds: u128 },
    #[error("Eip2930 transaction is enabled after berlin hardfork.")]
    TransactionEip2930Disabled,
    #[error("Old legacy transaction before Spurious Dragon should not have chain_id.")]
    TransactionOldLegacyChainId,
    #[error("Eip2930 transaction is enabled after london hardfork.")]
    TransactionEip1559Disabled,
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
}
