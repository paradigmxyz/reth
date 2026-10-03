use crate::tree::{
    merkleize_fixed, merkleize_progressive, mix_in_length, progressive_byte_list, RetainedNode,
    TreeConstructionError,
};
use alloy_consensus::{BlockHeader, Transaction, TxType};
use alloy_primitives::{Address, Bytes, Log as ExecutionLog, B256};
use reth_ethereum_primitives::{Block, Receipt as StoredReceipt, TransactionSigned};
use reth_primitives_traits::RecoveredBlock;
use ssz::{SszEncoder, BYTES_PER_LENGTH_OFFSET, MAX_LENGTH_VALUE};
use std::fmt;
use tree_hash::TreeHash;

pub const MAX_TOPICS_PER_LOG: usize = 4;

pub const BASIC_RECEIPT_SELECTOR: u8 = 0x01;
pub const CREATE_RECEIPT_SELECTOR: u8 = 0x02;
pub const SET_CODE_RECEIPT_SELECTOR: u8 = 0x03;

pub const BASIC_RECEIPT_ACTIVE_FIELDS: [bool; 5] = [true, true, false, true, true];
pub const CREATE_RECEIPT_ACTIVE_FIELDS: [bool; 5] = [true, true, true, true, true];
pub const SET_CODE_RECEIPT_ACTIVE_FIELDS: [bool; 6] = [true, true, false, true, true, true];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Log {
    address: Address,
    topics: Vec<B256>,
    data: Bytes,
}

impl Log {
    pub fn new(
        address: Address,
        topics: Vec<B256>,
        data: Bytes,
    ) -> Result<Self, ReceiptConstructionError> {
        if topics.len() > MAX_TOPICS_PER_LOG {
            return Err(ReceiptConstructionError::TooManyTopics {
                actual: topics.len(),
                max: MAX_TOPICS_PER_LOG,
            });
        }

        Ok(Self { address, topics, data })
    }

    pub const fn address(&self) -> Address {
        self.address
    }

    pub fn topics(&self) -> &[B256] {
        &self.topics
    }

    pub const fn data(&self) -> &Bytes {
        &self.data
    }
}

impl TryFrom<&ExecutionLog> for Log {
    type Error = ReceiptConstructionError;

    fn try_from(log: &ExecutionLog) -> Result<Self, Self::Error> {
        Self::new(log.address, log.data.topics().to_vec(), log.data.data.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BasicReceipt {
    pub from_: Address,
    pub gas_used: u64,
    pub logs: Vec<Log>,
    pub status: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateReceipt {
    pub from_: Address,
    pub gas_used: u64,
    pub contract_address: Address,
    pub logs: Vec<Log>,
    pub status: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetCodeReceipt {
    pub from_: Address,
    pub gas_used: u64,
    pub logs: Vec<Log>,
    pub status: bool,
    pub authorities: Vec<Address>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Receipt {
    Basic(BasicReceipt),
    Create(CreateReceipt),
    SetCode(SetCodeReceipt),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipts(Vec<Receipt>);

impl Receipts {
    pub const fn new(receipts: Vec<Receipt>) -> Self {
        Self(receipts)
    }

    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub const fn len(&self) -> usize {
        self.0.len()
    }

    pub fn get(&self, index: usize) -> Option<&Receipt> {
        self.0.get(index)
    }

    pub fn as_slice(&self) -> &[Receipt] {
        &self.0
    }
}

impl Receipt {
    pub const fn selector(&self) -> u8 {
        match self {
            Self::Basic(_) => BASIC_RECEIPT_SELECTOR,
            Self::Create(_) => CREATE_RECEIPT_SELECTOR,
            Self::SetCode(_) => SET_CODE_RECEIPT_SELECTOR,
        }
    }

    pub const fn active_fields(&self) -> &'static [bool] {
        match self {
            Self::Basic(_) => &BASIC_RECEIPT_ACTIVE_FIELDS,
            Self::Create(_) => &CREATE_RECEIPT_ACTIVE_FIELDS,
            Self::SetCode(_) => &SET_CODE_RECEIPT_ACTIVE_FIELDS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceiptSerializationError {
    ArithmeticOverflow { context: &'static str },
    MaximumLengthExceeded { context: &'static str, length: usize },
}

impl fmt::Display for ReceiptSerializationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ArithmeticOverflow { context } => {
                write!(formatter, "SSZ length arithmetic overflow while encoding {context}")
            }
            Self::MaximumLengthExceeded { context, length } => write!(
                formatter,
                "SSZ encoding for {context} is {length} bytes, maximum is {MAX_LENGTH_VALUE}"
            ),
        }
    }
}

impl std::error::Error for ReceiptSerializationError {}

#[derive(Debug)]
pub enum Eip6466SnapshotError {
    MissingData(ReceiptConstructionError),
    Conversion(ReceiptConstructionError),
    Serialization(ReceiptSerializationError),
    Tree(TreeConstructionError),
}

impl Eip6466SnapshotError {
    fn from_construction(error: ReceiptConstructionError) -> Self {
        if error.is_missing_data() {
            Self::MissingData(error)
        } else {
            Self::Conversion(error)
        }
    }
}

impl fmt::Display for Eip6466SnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingData(error) => write!(formatter, "missing EIP-6466 data: {error}"),
            Self::Conversion(error) => write!(formatter, "EIP-6466 conversion failed: {error}"),
            Self::Serialization(error) => {
                write!(formatter, "EIP-6466 serialization failed: {error}")
            }
            Self::Tree(error) => write!(formatter, "EIP-6466 tree construction failed: {error}"),
        }
    }
}

impl std::error::Error for Eip6466SnapshotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingData(error) | Self::Conversion(error) => Some(error),
            Self::Serialization(error) => Some(error),
            Self::Tree(error) => Some(error),
        }
    }
}

impl From<ReceiptSerializationError> for Eip6466SnapshotError {
    fn from(error: ReceiptSerializationError) -> Self {
        Self::Serialization(error)
    }
}

impl From<TreeConstructionError> for Eip6466SnapshotError {
    fn from(error: TreeConstructionError) -> Self {
        Self::Tree(error)
    }
}

#[derive(Debug)]
pub struct Eip6466ReceiptSnapshot {
    receipts: Receipts,
    serialized: Bytes,
    tree: RetainedNode,
}

impl Eip6466ReceiptSnapshot {
    pub fn build(receipts: Receipts) -> Result<Self, Eip6466SnapshotError> {
        let serialized = Bytes::from(serialize_receipts(&receipts)?);
        let tree = build_receipts_tree(&receipts)?;

        Ok(Self { receipts, serialized, tree })
    }

    pub fn from_block(
        block: &RecoveredBlock<Block>,
        stored_receipts: &[StoredReceipt],
        authorization_outcomes: &[Option<Vec<AuthorizationOutcome>>],
        eip658_active: bool,
    ) -> Result<Self, Eip6466SnapshotError> {
        let receipts =
            receipts_from_block(block, stored_receipts, authorization_outcomes, eip658_active)
                .map_err(Eip6466SnapshotError::from_construction)?;

        Self::build(receipts)
    }

    pub const fn receipts(&self) -> &Receipts {
        &self.receipts
    }

    pub const fn serialized(&self) -> &Bytes {
        &self.serialized
    }

    pub const fn tree(&self) -> &RetainedNode {
        &self.tree
    }

    pub const fn root(&self) -> B256 {
        self.tree.root()
    }
}

impl Receipt {
    pub fn to_ssz_bytes(&self) -> Result<Vec<u8>, ReceiptSerializationError> {
        serialize_receipt(self)
    }
}

impl Receipts {
    pub fn to_ssz_bytes(&self) -> Result<Vec<u8>, ReceiptSerializationError> {
        serialize_receipts(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorizationOutcome {
    Success(Address),
    Failure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReceiptKind {
    Basic,
    Create,
    SetCode,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReceiptConstructionError {
    InputCountMismatch { transactions: usize, receipts: usize, authorization_outcomes: usize },
    PreEip658Block,
    HeaderGasUsedExceedsLimit { gas_used: u64, gas_limit: u64 },
    TransactionTypeMismatch { index: usize, transaction: TxType, receipt: TxType },
    DecreasingCumulativeGas { index: usize, previous: u64, current: u64 },
    CumulativeGasExceedsBlockLimit { index: usize, cumulative: u64, gas_limit: u64 },
    BlockGasUsedMismatch { header: u64, receipts: u64 },
    UnsupportedTransaction { tx_type: TxType, is_create: bool },
    MissingAuthorizationList,
    MissingAuthorizationOutcomes,
    UnexpectedAuthorizationOutcomes { tx_type: TxType },
    AuthorizationOutcomeCountMismatch { expected: usize, actual: usize },
    TooManyTopics { actual: usize, max: usize },
}

impl fmt::Display for ReceiptConstructionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputCountMismatch { transactions, receipts, authorization_outcomes } => write!(
                formatter,
                "input count mismatch: {transactions} transactions, {receipts} receipts, \
                 {authorization_outcomes} authorization outcome entries"
            ),
            Self::PreEip658Block => {
                formatter.write_str("pre-EIP-658 receipts do not contain explicit status")
            }
            Self::HeaderGasUsedExceedsLimit { gas_used, gas_limit } => {
                write!(formatter, "block header gas used {gas_used} exceeds gas limit {gas_limit}")
            }
            Self::TransactionTypeMismatch { index, transaction, receipt } => write!(
                formatter,
                "transaction type mismatch at index {index}: \
                 transaction {transaction:?}, receipt {receipt:?}"
            ),
            Self::DecreasingCumulativeGas { index, previous, current } => write!(
                formatter,
                "cumulative gas decreased at index {index}: \
                 previous {previous}, current {current}"
            ),
            Self::CumulativeGasExceedsBlockLimit { index, cumulative, gas_limit } => write!(
                formatter,
                "cumulative gas {cumulative} at index {index} exceeds block gas limit {gas_limit}"
            ),
            Self::BlockGasUsedMismatch { header, receipts } => write!(
                formatter,
                "block gas used mismatch: header {header}, final receipt cumulative gas {receipts}"
            ),
            Self::UnsupportedTransaction { tx_type, is_create } => write!(
                formatter,
                "unsupported transaction type and kind: {tx_type:?}, create={is_create}"
            ),
            Self::MissingAuthorizationList => {
                formatter.write_str("set-code transaction is missing its authorization list")
            }
            Self::MissingAuthorizationOutcomes => formatter
                .write_str("set-code transaction is missing controlled authorization outcomes"),
            Self::UnexpectedAuthorizationOutcomes { tx_type } => write!(
                formatter,
                "authorization outcomes were supplied for non-set-code transaction {tx_type:?}"
            ),
            Self::AuthorizationOutcomeCountMismatch { expected, actual } => write!(
                formatter,
                "authorization outcome count mismatch: expected {expected}, got {actual}"
            ),
            Self::TooManyTopics { actual, max } => {
                write!(formatter, "log has {actual} topics, maximum is {max}")
            }
        }
    }
}

impl std::error::Error for ReceiptConstructionError {}

impl ReceiptConstructionError {
    fn is_missing_data(&self) -> bool {
        match self {
            Self::InputCountMismatch { transactions, receipts, authorization_outcomes } => {
                receipts < transactions || authorization_outcomes < transactions
            }
            Self::PreEip658Block |
            Self::MissingAuthorizationList |
            Self::MissingAuthorizationOutcomes => true,
            Self::AuthorizationOutcomeCountMismatch { expected, actual } => actual < expected,
            Self::HeaderGasUsedExceedsLimit { .. } |
            Self::TransactionTypeMismatch { .. } |
            Self::DecreasingCumulativeGas { .. } |
            Self::CumulativeGasExceedsBlockLimit { .. } |
            Self::BlockGasUsedMismatch { .. } |
            Self::UnsupportedTransaction { .. } |
            Self::UnexpectedAuthorizationOutcomes { .. } |
            Self::TooManyTopics { .. } => false,
        }
    }
}

pub fn receipt_from_transaction(
    transaction: &TransactionSigned,
    from_: Address,
    gas_used: u64,
    logs: Vec<Log>,
    status: bool,
    authorization_outcomes: Option<&[AuthorizationOutcome]>,
) -> Result<Receipt, ReceiptConstructionError> {
    let tx_type = transaction.tx_type();
    let kind = classify_receipt(tx_type, transaction.is_create())?;

    match kind {
        ReceiptKind::Basic => {
            reject_unexpected_authorization_outcomes(tx_type, authorization_outcomes)?;

            Ok(Receipt::Basic(BasicReceipt { from_, gas_used, logs, status }))
        }
        ReceiptKind::Create => {
            reject_unexpected_authorization_outcomes(tx_type, authorization_outcomes)?;

            Ok(Receipt::Create(CreateReceipt {
                from_,
                gas_used,
                contract_address: from_.create(transaction.nonce()),
                logs,
                status,
            }))
        }
        ReceiptKind::SetCode => {
            let authorizations = transaction
                .authorization_list()
                .ok_or(ReceiptConstructionError::MissingAuthorizationList)?;
            let authorities =
                authorization_addresses(authorizations.len(), authorization_outcomes)?;

            Ok(Receipt::SetCode(SetCodeReceipt { from_, gas_used, logs, status, authorities }))
        }
    }
}

pub fn receipts_from_block(
    block: &RecoveredBlock<Block>,
    stored_receipts: &[StoredReceipt],
    authorization_outcomes: &[Option<Vec<AuthorizationOutcome>>],
    eip658_active: bool,
) -> Result<Receipts, ReceiptConstructionError> {
    let transaction_count = block.body().transactions.len();

    if transaction_count != stored_receipts.len() ||
        transaction_count != authorization_outcomes.len()
    {
        return Err(ReceiptConstructionError::InputCountMismatch {
            transactions: transaction_count,
            receipts: stored_receipts.len(),
            authorization_outcomes: authorization_outcomes.len(),
        });
    }

    if !eip658_active {
        return Err(ReceiptConstructionError::PreEip658Block);
    }

    let block_gas_used = block.header().gas_used();
    let block_gas_limit = block.header().gas_limit();

    if block_gas_used > block_gas_limit {
        return Err(ReceiptConstructionError::HeaderGasUsedExceedsLimit {
            gas_used: block_gas_used,
            gas_limit: block_gas_limit,
        });
    }

    let mut previous_cumulative_gas = 0;
    let mut converted = Vec::with_capacity(transaction_count);

    for (index, ((transaction, stored_receipt), outcomes)) in
        block.transactions_recovered().zip(stored_receipts).zip(authorization_outcomes).enumerate()
    {
        let transaction_type = transaction.tx_type();

        if transaction_type != stored_receipt.tx_type {
            return Err(ReceiptConstructionError::TransactionTypeMismatch {
                index,
                transaction: transaction_type,
                receipt: stored_receipt.tx_type,
            });
        }

        let cumulative_gas = stored_receipt.cumulative_gas_used;

        if cumulative_gas > block_gas_limit {
            return Err(ReceiptConstructionError::CumulativeGasExceedsBlockLimit {
                index,
                cumulative: cumulative_gas,
                gas_limit: block_gas_limit,
            });
        }

        let gas_used = cumulative_gas.checked_sub(previous_cumulative_gas).ok_or(
            ReceiptConstructionError::DecreasingCumulativeGas {
                index,
                previous: previous_cumulative_gas,
                current: cumulative_gas,
            },
        )?;

        previous_cumulative_gas = cumulative_gas;

        let logs = stored_receipt.logs.iter().map(Log::try_from).collect::<Result<Vec<_>, _>>()?;

        let from_ = transaction.signer();
        let signed_transaction = *transaction.inner();

        converted.push(receipt_from_transaction(
            signed_transaction,
            from_,
            gas_used,
            logs,
            stored_receipt.success,
            outcomes.as_deref(),
        )?);
    }

    if previous_cumulative_gas != block_gas_used {
        return Err(ReceiptConstructionError::BlockGasUsedMismatch {
            header: block_gas_used,
            receipts: previous_cumulative_gas,
        });
    }

    Ok(Receipts(converted))
}

const fn classify_receipt(
    tx_type: TxType,
    is_create: bool,
) -> Result<ReceiptKind, ReceiptConstructionError> {
    match (tx_type, is_create) {
        (TxType::Legacy | TxType::Eip2930 | TxType::Eip1559 | TxType::Eip4844, false) => {
            Ok(ReceiptKind::Basic)
        }
        (TxType::Legacy | TxType::Eip2930 | TxType::Eip1559, true) => Ok(ReceiptKind::Create),
        (TxType::Eip7702, false) => Ok(ReceiptKind::SetCode),
        (TxType::Eip4844 | TxType::Eip7702, true) => {
            Err(ReceiptConstructionError::UnsupportedTransaction { tx_type, is_create })
        }
    }
}

const fn reject_unexpected_authorization_outcomes(
    tx_type: TxType,
    authorization_outcomes: Option<&[AuthorizationOutcome]>,
) -> Result<(), ReceiptConstructionError> {
    if authorization_outcomes.is_some() {
        return Err(ReceiptConstructionError::UnexpectedAuthorizationOutcomes { tx_type });
    }

    Ok(())
}

fn authorization_addresses(
    expected: usize,
    authorization_outcomes: Option<&[AuthorizationOutcome]>,
) -> Result<Vec<Address>, ReceiptConstructionError> {
    let authorization_outcomes =
        authorization_outcomes.ok_or(ReceiptConstructionError::MissingAuthorizationOutcomes)?;

    if authorization_outcomes.len() != expected {
        return Err(ReceiptConstructionError::AuthorizationOutcomeCountMismatch {
            expected,
            actual: authorization_outcomes.len(),
        });
    }

    Ok(authorization_outcomes
        .iter()
        .map(|outcome| match outcome {
            AuthorizationOutcome::Success(address) => *address,
            AuthorizationOutcome::Failure => Address::ZERO,
        })
        .collect())
}

fn checked_add_length(
    context: &'static str,
    left: usize,
    right: usize,
) -> Result<usize, ReceiptSerializationError> {
    left.checked_add(right).ok_or(ReceiptSerializationError::ArithmeticOverflow { context })
}

fn checked_mul_length(
    context: &'static str,
    left: usize,
    right: usize,
) -> Result<usize, ReceiptSerializationError> {
    left.checked_mul(right).ok_or(ReceiptSerializationError::ArithmeticOverflow { context })
}

const fn checked_serialized_length(
    context: &'static str,
    length: usize,
) -> Result<usize, ReceiptSerializationError> {
    if length > MAX_LENGTH_VALUE {
        return Err(ReceiptSerializationError::MaximumLengthExceeded { context, length });
    }

    Ok(length)
}

fn checked_container_length(
    context: &'static str,
    fixed_length: usize,
    variable_lengths: impl IntoIterator<Item = Result<usize, ReceiptSerializationError>>,
) -> Result<usize, ReceiptSerializationError> {
    let total = variable_lengths
        .into_iter()
        .try_fold(fixed_length, |total, length| checked_add_length(context, total, length?))?;

    checked_serialized_length(context, total)
}

fn checked_variable_sequence_length(
    context: &'static str,
    count: usize,
    item_lengths: impl IntoIterator<Item = Result<usize, ReceiptSerializationError>>,
) -> Result<usize, ReceiptSerializationError> {
    let fixed_length = checked_mul_length(context, count, BYTES_PER_LENGTH_OFFSET)?;
    checked_container_length(context, fixed_length, item_lengths)
}

fn checked_log_length(log: &Log) -> Result<usize, ReceiptSerializationError> {
    let topics_length = checked_mul_length("log topics", log.topics.len(), 32)?;
    checked_serialized_length("log data", log.data.len())?;

    checked_container_length("log", 28, [Ok(topics_length), Ok(log.data.len())])
}

fn checked_logs_length(logs: &[Log]) -> Result<usize, ReceiptSerializationError> {
    checked_variable_sequence_length(
        "receipt logs",
        logs.len(),
        logs.iter().map(checked_log_length),
    )
}

fn checked_basic_receipt_length(
    receipt: &BasicReceipt,
) -> Result<usize, ReceiptSerializationError> {
    checked_container_length("basic receipt", 33, [checked_logs_length(&receipt.logs)])
}

fn checked_create_receipt_length(
    receipt: &CreateReceipt,
) -> Result<usize, ReceiptSerializationError> {
    checked_container_length("create receipt", 53, [checked_logs_length(&receipt.logs)])
}

fn checked_set_code_receipt_length(
    receipt: &SetCodeReceipt,
) -> Result<usize, ReceiptSerializationError> {
    let authorities_length =
        checked_mul_length("set-code authorities", receipt.authorities.len(), 20)?;

    checked_container_length(
        "set-code receipt",
        37,
        [checked_logs_length(&receipt.logs), Ok(authorities_length)],
    )
}

fn checked_receipt_length(receipt: &Receipt) -> Result<usize, ReceiptSerializationError> {
    let body_length = match receipt {
        Receipt::Basic(receipt) => checked_basic_receipt_length(receipt)?,
        Receipt::Create(receipt) => checked_create_receipt_length(receipt)?,
        Receipt::SetCode(receipt) => checked_set_code_receipt_length(receipt)?,
    };

    checked_container_length("compatible-union receipt", 1, [Ok(body_length)])
}

fn checked_receipts_length(receipts: &Receipts) -> Result<usize, ReceiptSerializationError> {
    checked_variable_sequence_length(
        "receipt list",
        receipts.len(),
        receipts.as_slice().iter().map(checked_receipt_length),
    )
}

fn append_variable_sequence<T>(values: &[T], bytes: &mut Vec<u8>, append: fn(&T, &mut Vec<u8>)) {
    let mut encoder = SszEncoder::container(bytes, values.len() * BYTES_PER_LENGTH_OFFSET);

    for value in values {
        encoder.append_parameterized(false, |variable_bytes| append(value, variable_bytes));
    }

    encoder.finalize();
}

fn append_log(log: &Log, bytes: &mut Vec<u8>) {
    let mut encoder = SszEncoder::container(bytes, 28);
    encoder.append(&log.address);
    encoder.append(&log.topics);
    encoder.append(&log.data);
    encoder.finalize();
}

fn append_logs(logs: &[Log], bytes: &mut Vec<u8>) {
    append_variable_sequence(logs, bytes, append_log);
}

fn append_basic_receipt(receipt: &BasicReceipt, bytes: &mut Vec<u8>) {
    let mut encoder = SszEncoder::container(bytes, 33);
    encoder.append(&receipt.from_);
    encoder.append(&receipt.gas_used);
    encoder
        .append_parameterized(false, |variable_bytes| append_logs(&receipt.logs, variable_bytes));
    encoder.append(&receipt.status);
    encoder.finalize();
}

fn append_create_receipt(receipt: &CreateReceipt, bytes: &mut Vec<u8>) {
    let mut encoder = SszEncoder::container(bytes, 53);
    encoder.append(&receipt.from_);
    encoder.append(&receipt.gas_used);
    encoder.append(&receipt.contract_address);
    encoder
        .append_parameterized(false, |variable_bytes| append_logs(&receipt.logs, variable_bytes));
    encoder.append(&receipt.status);
    encoder.finalize();
}

fn append_set_code_receipt(receipt: &SetCodeReceipt, bytes: &mut Vec<u8>) {
    let mut encoder = SszEncoder::container(bytes, 37);
    encoder.append(&receipt.from_);
    encoder.append(&receipt.gas_used);
    encoder
        .append_parameterized(false, |variable_bytes| append_logs(&receipt.logs, variable_bytes));
    encoder.append(&receipt.status);
    encoder.append(&receipt.authorities);
    encoder.finalize();
}

fn append_receipt(receipt: &Receipt, bytes: &mut Vec<u8>) {
    bytes.push(receipt.selector());

    match receipt {
        Receipt::Basic(receipt) => append_basic_receipt(receipt, bytes),
        Receipt::Create(receipt) => append_create_receipt(receipt, bytes),
        Receipt::SetCode(receipt) => append_set_code_receipt(receipt, bytes),
    }
}

fn append_receipts(receipts: &Receipts, bytes: &mut Vec<u8>) {
    append_variable_sequence(receipts.as_slice(), bytes, append_receipt);
}

fn serialize_receipt(receipt: &Receipt) -> Result<Vec<u8>, ReceiptSerializationError> {
    let length = checked_receipt_length(receipt)?;
    let mut bytes = Vec::with_capacity(length);
    append_receipt(receipt, &mut bytes);
    debug_assert_eq!(bytes.len(), length);

    Ok(bytes)
}

fn serialize_receipts(receipts: &Receipts) -> Result<Vec<u8>, ReceiptSerializationError> {
    let length = checked_receipts_length(receipts)?;
    let mut bytes = Vec::with_capacity(length);
    append_receipts(receipts, &mut bytes);
    debug_assert_eq!(bytes.len(), length);

    Ok(bytes)
}

fn mix_in_active_fields(contents: RetainedNode, active_fields: &[bool]) -> RetainedNode {
    let mut chunk = [0_u8; 32];

    for (index, active) in active_fields.iter().copied().enumerate() {
        if active {
            chunk[index / 8] |= 1 << (index % 8);
        }
    }

    RetainedNode::pair(contents, RetainedNode::leaf(B256::from(chunk)))
}

fn mix_in_selector(contents: RetainedNode, selector: u8) -> RetainedNode {
    let mut chunk = [0_u8; 32];
    chunk[0] = selector;

    RetainedNode::pair(contents, RetainedNode::leaf(B256::from(chunk)))
}

fn build_receipts_tree(receipts: &Receipts) -> Result<RetainedNode, TreeConstructionError> {
    let nodes =
        receipts.as_slice().iter().map(build_receipt_tree).collect::<Result<Vec<_>, _>>()?;

    Ok(mix_in_length(merkleize_progressive(nodes)?, receipts.len()))
}

fn build_receipt_tree(receipt: &Receipt) -> Result<RetainedNode, TreeConstructionError> {
    Ok(mix_in_selector(build_receipt_container_tree(receipt)?, receipt.selector()))
}

fn build_receipt_container_tree(receipt: &Receipt) -> Result<RetainedNode, TreeConstructionError> {
    let (fields, active_fields) = match receipt {
        Receipt::Basic(receipt) => (
            vec![
                RetainedNode::leaf(receipt.from_.tree_hash_root()),
                RetainedNode::leaf(receipt.gas_used.tree_hash_root()),
                RetainedNode::zero(),
                build_logs_tree(&receipt.logs)?,
                RetainedNode::leaf(receipt.status.tree_hash_root()),
            ],
            &BASIC_RECEIPT_ACTIVE_FIELDS[..],
        ),
        Receipt::Create(receipt) => (
            vec![
                RetainedNode::leaf(receipt.from_.tree_hash_root()),
                RetainedNode::leaf(receipt.gas_used.tree_hash_root()),
                RetainedNode::leaf(receipt.contract_address.tree_hash_root()),
                build_logs_tree(&receipt.logs)?,
                RetainedNode::leaf(receipt.status.tree_hash_root()),
            ],
            &CREATE_RECEIPT_ACTIVE_FIELDS[..],
        ),
        Receipt::SetCode(receipt) => (
            vec![
                RetainedNode::leaf(receipt.from_.tree_hash_root()),
                RetainedNode::leaf(receipt.gas_used.tree_hash_root()),
                RetainedNode::zero(),
                build_logs_tree(&receipt.logs)?,
                RetainedNode::leaf(receipt.status.tree_hash_root()),
                build_authorities_tree(&receipt.authorities)?,
            ],
            &SET_CODE_RECEIPT_ACTIVE_FIELDS[..],
        ),
    };

    Ok(mix_in_active_fields(merkleize_progressive(fields)?, active_fields))
}

fn build_logs_tree(logs: &[Log]) -> Result<RetainedNode, TreeConstructionError> {
    let nodes = logs.iter().map(build_log_tree).collect::<Result<Vec<_>, _>>()?;

    Ok(mix_in_length(merkleize_progressive(nodes)?, logs.len()))
}

fn build_log_tree(log: &Log) -> Result<RetainedNode, TreeConstructionError> {
    merkleize_fixed(
        vec![
            RetainedNode::leaf(log.address.tree_hash_root()),
            build_topics_tree(&log.topics)?,
            progressive_byte_list(log.data.as_ref())?,
        ],
        4,
    )
}

fn build_topics_tree(topics: &[B256]) -> Result<RetainedNode, TreeConstructionError> {
    let nodes = topics.iter().map(|topic| RetainedNode::leaf(topic.tree_hash_root())).collect();

    Ok(mix_in_length(merkleize_fixed(nodes, MAX_TOPICS_PER_LOG)?, topics.len()))
}

fn build_authorities_tree(authorities: &[Address]) -> Result<RetainedNode, TreeConstructionError> {
    let nodes = authorities
        .iter()
        .map(|authority| RetainedNode::leaf(authority.tree_hash_root()))
        .collect();

    Ok(mix_in_length(merkleize_progressive(nodes)?, authorities.len()))
}

#[cfg(test)]
mod tests;
