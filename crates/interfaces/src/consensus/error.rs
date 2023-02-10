use reth_primitives::{Address, BlockHash, BlockNumber, Bytes, H256, U256};
use std::fmt::Debug;

/// Consensus Errors
#[allow(missing_docs)]
#[derive(thiserror::Error, Debug, PartialEq, Eq, Clone)]
pub enum Error {
    #[error("Block used gas ({gas_used:?}) is greater than gas limit ({gas_limit:?}).")]
    HeaderGasUsedExceedsGasLimit { gas_used: u64, gas_limit: u64 },
    #[error("Block ommer hash ({got:?}) is different then expected: ({expected:?})")]
    BodyOmmersHashDiff { got: H256, expected: H256 },
    #[error("Block state root ({got:?}) is different then expected: ({expected:?})")]
    BodyStateRootDiff { got: H256, expected: H256 },
    #[error("Block transaction root ({got:?}) is different then expected: ({expected:?})")]
    BodyTransactionRootDiff { got: H256, expected: H256 },
    #[error("Block receipts root ({got:?}) is different then expected: ({expected:?}).")]
    BodyReceiptsRootDiff { got: H256, expected: H256 },
    #[error("Block #{number} ({hash:?}) is already known.")]
    BlockKnown { hash: BlockHash, number: BlockNumber },
    #[error("Block parent {hash:?} is not known.")]
    ParentUnknown { hash: BlockHash },
    #[error("Block number #{block_number:?} is not consistent with parent block number #{parent_block_number:?}")]
    ParentBlockNumberMismatch { parent_block_number: BlockNumber, block_number: BlockNumber },
    #[error(
        "Parent hash mismatch for block #{block_number}. Expected: {expected:?}. Received: {received:?}"
    )]
    ParentHashMismatch { block_number: BlockNumber, expected: H256, received: H256 },
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
    #[error("Base fee missing for block.")]
    BaseFeeMissing,
    #[error("Encountered unexpected base fee.")]
    UnexpectedBaseFee,
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
    #[error(transparent)]
    Clique(#[from] CliqueError),
}

/// Clique consensus error.
/// https://github.com/ethereum/go-ethereum/blob/d0a4989a8def7e6bad182d1513e8d4a093c1672d/consensus/clique/clique.go#L72-L140
#[allow(missing_docs)]
#[derive(thiserror::Error, Debug, PartialEq, Eq, Clone)]
pub enum CliqueError {
    #[error("Clique config is missing")]
    Config,
    #[error("Beneficiary in checkpoint block non-zero: {beneficiary}")]
    CheckpointBeneficiary { beneficiary: Address },
    #[error("Vote nonce in checkpoint block non-zero: {nonce}")]
    CheckpointVote { nonce: u64 },
    #[error("Vote nonce not 0x00..0 or 0xff..f: {nonce}")]
    InvalidVote { nonce: u64 },
    #[error("Vanity prefix missing from extra data: {extra_data}")]
    MissingVanity { extra_data: Bytes },
    #[error("Signature prefix missing from extra data: {extra_data}")]
    MissingSignature { extra_data: Bytes },
    #[error("Non-checkpoint block contains extra signer list: {extra_data}")]
    ExtraSignerList { extra_data: Bytes },
    #[error("Invalid signer list on checkpoint block: {extra_data}")]
    CheckpointSigners { extra_data: Bytes },
    #[error("Non-zero mix hash: {mix_hash}")]
    NonZeroMixHash { mix_hash: H256 },
    #[error("Invalid difficulty: {difficulty}")]
    Difficulty { difficulty: U256 },
    #[error("Invalid child header timestamp. Expected at least: {expected_at_least}. Received: {received}")]
    Timestamp { expected_at_least: u64, received: u64 },
    #[error("Failed to recover header signer. Signature: {signature:?}. Hash: {hash:?}")]
    HeaderSignerRecovery { signature: [u8; 65], hash: H256 },
    #[error("Invalid voting chain. Expected: {expected}. Received: {received}")]
    InvalidVotingChain { expected: BlockNumber, received: BlockNumber },
    #[error("Unathorized signer {signer:?}")]
    UnauthorizedSigner { signer: Address },
    #[error("Recent signer {signer:?}")]
    RecentSigner { signer: Address },
}
