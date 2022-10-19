mod access_list;
mod signature;
mod tx_type;

use crate::{Address, Bytes, TxHash, U256};
pub use access_list::{AccessList, AccessListItem};
use bytes::Buf;
use ethers_core::utils::keccak256;
use reth_rlp::{length_of_length, Decodable, DecodeError, Encodable, Header, EMPTY_STRING_CODE};
use signature::Signature;
use std::ops::Deref;
pub use tx_type::TxType;

/// Raw Transaction.
/// Transaction type is introduced in EIP-2718: https://eips.ethereum.org/EIPS/eip-2718
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Transaction {
    /// Legacy transaciton.
    Legacy {
        /// Added as EIP-155: Simple replay attack protection
        chain_id: Option<u64>,
        /// A scalar value equal to the number of transactions sent by the sender; formally Tn.
        nonce: u64,
        /// A scalar value equal to the number of
        /// Wei to be paid per unit of gas for all computation
        /// costs incurred as a result of the execution of this transaction; formally Tp.
        gas_price: u64,
        /// A scalar value equal to the maximum
        /// amount of gas that should be used in executing
        /// this transaction. This is paid up-front, before any
        /// computation is done and may not be increased
        /// later; formally Tg.
        gas_limit: u64,
        /// The 160-bit address of the message call’s recipient or, for a contract creation
        /// transaction, ∅, used here to denote the only member of B0 ; formally Tt.
        to: TransactionKind,
        /// A scalar value equal to the number of Wei to
        /// be transferred to the message call’s recipient or,
        /// in the case of contract creation, as an endowment
        /// to the newly created account; formally Tv.
        value: U256,
        /// Input has two uses depending if transaction is Create or Call (if `to` field is None or
        /// Some). init: An unlimited size byte array specifying the
        /// EVM-code for the account initialisation procedure CREATE,
        /// data: An unlimited size byte array specifying the
        /// input data of the message call, formally Td.
        input: Bytes,
    },
    /// Transaction with AccessList. https://eips.ethereum.org/EIPS/eip-2930
    Eip2930 {
        /// Added as EIP-155: Simple replay attack protection
        chain_id: u64,
        /// A scalar value equal to the number of transactions sent by the sender; formally Tn.
        nonce: u64,
        /// A scalar value equal to the number of
        /// Wei to be paid per unit of gas for all computation
        /// costs incurred as a result of the execution of this transaction; formally Tp.
        gas_price: u64,
        /// A scalar value equal to the maximum
        /// amount of gas that should be used in executing
        /// this transaction. This is paid up-front, before any
        /// computation is done and may not be increased
        /// later; formally Tg.
        gas_limit: u64,
        /// The 160-bit address of the message call’s recipient or, for a contract creation
        /// transaction, ∅, used here to denote the only member of B0 ; formally Tt.
        to: TransactionKind,
        /// A scalar value equal to the number of Wei to
        /// be transferred to the message call’s recipient or,
        /// in the case of contract creation, as an endowment
        /// to the newly created account; formally Tv.
        value: U256,
        /// Input has two uses depending if transaction is Create or Call (if `to` field is None or
        /// Some). init: An unlimited size byte array specifying the
        /// EVM-code for the account initialisation procedure CREATE,
        /// data: An unlimited size byte array specifying the
        /// input data of the message call, formally Td.
        input: Bytes,
        /// The accessList specifies a list of addresses and storage keys;
        /// these addresses and storage keys are added into the `accessed_addresses`
        /// and `accessed_storage_keys` global sets (introduced in EIP-2929).
        /// A gas cost is charged, though at a discount relative to the cost of
        /// accessing outside the list.
        access_list: AccessList,
    },
    /// Transaction with priority fee. https://eips.ethereum.org/EIPS/eip-1559
    Eip1559 {
        /// Added as EIP-155: Simple replay attack protection
        chain_id: u64,
        /// A scalar value equal to the number of transactions sent by the sender; formally Tn.
        nonce: u64,
        /// A scalar value equal to the maximum
        /// amount of gas that should be used in executing
        /// this transaction. This is paid up-front, before any
        /// computation is done and may not be increased
        /// later; formally Tg.
        gas_limit: u64,
        /// A scalar value equal to the maximum
        /// amount of gas that should be used in executing
        /// this transaction. This is paid up-front, before any
        /// computation is done and may not be increased
        /// later; formally Tg.
        max_fee_per_gas: u64,
        /// Max Priority fee that transaction is paying
        max_priority_fee_per_gas: u64,
        /// The 160-bit address of the message call’s recipient or, for a contract creation
        /// transaction, ∅, used here to denote the only member of B0 ; formally Tt.
        to: TransactionKind,
        /// A scalar value equal to the number of Wei to
        /// be transferred to the message call’s recipient or,
        /// in the case of contract creation, as an endowment
        /// to the newly created account; formally Tv.
        value: U256,
        /// Input has two uses depending if transaction is Create or Call (if `to` field is None or
        /// Some). init: An unlimited size byte array specifying the
        /// EVM-code for the account initialisation procedure CREATE,
        /// data: An unlimited size byte array specifying the
        /// input data of the message call, formally Td.
        input: Bytes,
        /// The accessList specifies a list of addresses and storage keys;
        /// these addresses and storage keys are added into the `accessed_addresses`
        /// and `accessed_storage_keys` global sets (introduced in EIP-2929).
        /// A gas cost is charged, though at a discount relative to the cost of
        /// accessing outside the list.
        access_list: AccessList,
    },
}

impl Transaction {
    /// Heavy operation that return hash over rlp encoded transaction.
    /// It is only used for signature signing.
    pub fn signature_hash(&self) -> TxHash {
        let mut encoded = vec![];
        self.encode(&mut encoded);
        keccak256(encoded).into()
    }

    /// Sets the transaction's chain id to the provided value.
    pub fn set_chain_id(&mut self, chain_id: u64) {
        match self {
            Transaction::Legacy { chain_id: ref mut c, .. } => *c = Some(chain_id),
            Transaction::Eip2930 { chain_id: ref mut c, .. } => *c = chain_id,
            Transaction::Eip1559 { chain_id: ref mut c, .. } => *c = chain_id,
        }
    }

    /// Gets the transaction's [`TransactionKind`], which is the address of the recipient or
    /// [`TransactionKind::Create`] if the transaction is a contract creation.
    pub fn kind(&self) -> &TransactionKind {
        match self {
            Transaction::Legacy { to, .. } => to,
            Transaction::Eip2930 { to, .. } => to,
            Transaction::Eip1559 { to, .. } => to,
        }
    }

    /// Gets the transaction's value field.
    pub fn value(&self) -> &U256 {
        match self {
            Transaction::Legacy { value, .. } => value,
            Transaction::Eip2930 { value, .. } => value,
            Transaction::Eip1559 { value, .. } => value,
        }
    }

    /// Get the transaction's nonce.
    pub fn nonce(&self) -> u64 {
        match self {
            Transaction::Legacy { nonce, .. } => *nonce,
            Transaction::Eip2930 { nonce, .. } => *nonce,
            Transaction::Eip1559 { nonce, .. } => *nonce,
        }
    }

    /// Get the transaction's input field.
    pub fn input(&self) -> &Bytes {
        match self {
            Transaction::Legacy { input, .. } => input,
            Transaction::Eip2930 { input, .. } => input,
            Transaction::Eip1559 { input, .. } => input,
        }
    }

    /// Encodes individual transaction fields into the desired buffer, without a RLP header or
    /// EIP-155 fields.
    pub(crate) fn encode_inner(&self, out: &mut dyn bytes::BufMut) {
        match self {
            Transaction::Legacy { chain_id: _, nonce, gas_price, gas_limit, to, value, input } => {
                nonce.encode(out);
                gas_price.encode(out);
                gas_limit.encode(out);
                to.encode(out);
                value.encode(out);
                input.0.encode(out);
            }
            Transaction::Eip2930 {
                chain_id,
                nonce,
                gas_price,
                gas_limit,
                to,
                value,
                input,
                access_list,
            } => {
                out.put_u8(1);
                chain_id.encode(out);
                nonce.encode(out);
                gas_price.encode(out);
                gas_limit.encode(out);
                to.encode(out);
                value.encode(out);
                input.0.encode(out);
                access_list.encode(out);
            }
            Transaction::Eip1559 {
                chain_id,
                nonce,
                gas_limit,
                max_fee_per_gas,
                max_priority_fee_per_gas,
                to,
                value,
                input,
                access_list,
            } => {
                out.put_u8(2);
                chain_id.encode(out);
                nonce.encode(out);
                gas_limit.encode(out);
                max_fee_per_gas.encode(out);
                max_priority_fee_per_gas.encode(out);
                to.encode(out);
                value.encode(out);
                input.0.encode(out);
                access_list.encode(out);
            }
        }
    }

    /// Encodes EIP-155 arguments into the desired buffer. Only encodes values for legacy
    /// transactions.
    pub(crate) fn encode_eip155_fields(&self, out: &mut dyn bytes::BufMut) {
        // if this is a legacy transaction without a chain ID, it must be pre-EIP-155
        // and does not need to encode the chain ID for the signature hash encoding
        if let Transaction::Legacy { chain_id: Some(id), .. } = self {
            // EIP-155 encodes the chain ID and two zeroes
            id.encode(out);
            0x00u8.encode(out);
            0x00u8.encode(out);
        }
    }

    /// Outputs the length of EIP-155 fields. Only outputs a non-zero value for EIP-155 legacy
    /// transactions.
    pub(crate) fn eip155_fields_len(&self) -> usize {
        if let Transaction::Legacy { chain_id: Some(id), .. } = self {
            // EIP-155 encodes the chain ID and two zeroes, so we add 2 to the length of the chain
            // ID to get the length of all 3 fields
            // len(chain_id) + (0x00) + (0x00)
            id.length() + 2
        } else {
            // this is either a pre-EIP-155 legacy transaction or a typed transaction
            0
        }
    }

    /// Outputs the length of the transaction payload without the length of the RLP header or
    /// EIP-155 fields.
    pub(crate) fn payload_len(&self) -> usize {
        match self {
            Transaction::Legacy { chain_id: _, nonce, gas_price, gas_limit, to, value, input } => {
                let mut len = 0;
                len += nonce.length();
                len += gas_price.length();
                len += gas_limit.length();
                len += to.length();
                len += value.length();
                len += input.0.length();
                len
            }
            Transaction::Eip2930 {
                chain_id,
                nonce,
                gas_price,
                gas_limit,
                to,
                value,
                input,
                access_list,
            } => {
                let mut len = 0;
                len += chain_id.length();
                len += nonce.length();
                len += gas_price.length();
                len += gas_limit.length();
                len += to.length();
                len += value.length();
                len += input.0.length();
                len += access_list.length();
                // add 1 for the transaction type
                len + 1
            }
            Transaction::Eip1559 {
                chain_id,
                nonce,
                gas_limit,
                max_fee_per_gas,
                max_priority_fee_per_gas,
                to,
                value,
                input,
                access_list,
            } => {
                let mut len = 0;
                len += chain_id.length();
                len += nonce.length();
                len += gas_limit.length();
                len += max_fee_per_gas.length();
                len += max_priority_fee_per_gas.length();
                len += to.length();
                len += value.length();
                len += input.0.length();
                len += access_list.length();
                // add 1 for the transaction type
                len + 1
            }
        }
    }
}

/// This encodes the transaction _without_ the signature, and is only suitable for creating a hash
/// intended for signing.
impl Encodable for Transaction {
    fn length(&self) -> usize {
        let len = self.payload_len() + self.eip155_fields_len();
        len + length_of_length(len)
    }
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        let header = Header { list: true, payload_length: self.length() };
        header.encode(out);
        self.encode_inner(out);
        self.encode_eip155_fields(out);
    }
}

/// Whether or not the transaction is a contract creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransactionKind {
    /// A transaction that creates a contract.
    Create,
    /// A transaction that calls a contract or transfer.
    Call(Address),
}

impl Encodable for TransactionKind {
    fn length(&self) -> usize {
        match self {
            TransactionKind::Call(to) => to.length(),
            TransactionKind::Create => EMPTY_STRING_CODE.length(),
        }
    }
    fn encode(&self, out: &mut dyn reth_rlp::BufMut) {
        match self {
            TransactionKind::Call(to) => to.encode(out),
            TransactionKind::Create => EMPTY_STRING_CODE.encode(out),
        }
    }
}

impl Decodable for TransactionKind {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        if let Some(&first) = buf.first() {
            if first == EMPTY_STRING_CODE {
                buf.advance(1);
                Ok(TransactionKind::Create)
            } else {
                let addr = <Address as Decodable>::decode(buf)?;
                Ok(TransactionKind::Call(addr))
            }
        } else {
            Err(DecodeError::InputTooShort)
        }
    }
}

/// Signed transaction.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TransactionSigned {
    transaction: Transaction,
    hash: TxHash,
    signature: Signature,
}

impl AsRef<Transaction> for TransactionSigned {
    fn as_ref(&self) -> &Transaction {
        &self.transaction
    }
}

impl Deref for TransactionSigned {
    type Target = Transaction;

    fn deref(&self) -> &Self::Target {
        &self.transaction
    }
}

impl Encodable for TransactionSigned {
    fn length(&self) -> usize {
        let mut len = self.transaction.payload_len();
        if let Transaction::Legacy { chain_id: None, .. } = self.transaction {
            // if the transaction has no chain id then it is a pre-EIP-155 transaction
            len += self.signature.payload_len();
        } else {
            let id = match self.transaction {
                Transaction::Legacy { chain_id: Some(id), .. } => id,
                Transaction::Eip2930 { chain_id, .. } => chain_id,
                Transaction::Eip1559 { chain_id, .. } => chain_id,
                // we handled this case above
                _ => unreachable!(
                    "legacy transaction without chain id should have been handled above"
                ),
            };
            len += self.signature.eip155_payload_len(id);
        }

        // add the length of the RLP header
        len + length_of_length(len)
    }
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        let header = Header { list: true, payload_length: self.length() };
        header.encode(out);
        self.transaction.encode_inner(out);
        if let Transaction::Legacy { chain_id: None, .. } = self.transaction {
            // if the transaction has no chain id then it is a pre-EIP-155 transaction
            self.signature.encode_inner(out);
        } else {
            let id = match self.transaction {
                Transaction::Legacy { chain_id: Some(id), .. } => id,
                Transaction::Eip2930 { chain_id, .. } => chain_id,
                Transaction::Eip1559 { chain_id, .. } => chain_id,
                // we handled this case above
                _ => unreachable!(
                    "legacy transaction without chain id should have been handled above"
                ),
            };
            self.signature.encode_eip155_inner(out, id);
        }
    }
}

/// This `Decodable` implementation only supports decoding the transaction format sent over p2p.
impl Decodable for TransactionSigned {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        // keep this around so we can use it to calculate the hash
        let original_encoding = *buf;

        let header = Header::decode(buf)?;
        // if the transaction is encoded as a string then it is a typed transaction
        if !header.list {
            let tx_type = *buf
                .first()
                .ok_or(DecodeError::Custom("typed tx cannot be decoded from an empty slice"))?;
            buf.advance(1);
            // decode common fields
            let transaction = match tx_type {
                1 => Transaction::Eip2930 {
                    chain_id: Decodable::decode(buf)?,
                    nonce: Decodable::decode(buf)?,
                    gas_price: Decodable::decode(buf)?,
                    gas_limit: Decodable::decode(buf)?,
                    to: Decodable::decode(buf)?,
                    value: Decodable::decode(buf)?,
                    input: Bytes(Decodable::decode(buf)?),
                    access_list: Decodable::decode(buf)?,
                },
                2 => Transaction::Eip1559 {
                    chain_id: Decodable::decode(buf)?,
                    nonce: Decodable::decode(buf)?,
                    gas_limit: Decodable::decode(buf)?,
                    to: Decodable::decode(buf)?,
                    value: Decodable::decode(buf)?,
                    input: Bytes(Decodable::decode(buf)?),
                    access_list: Decodable::decode(buf)?,
                    max_fee_per_gas: Decodable::decode(buf)?,
                    max_priority_fee_per_gas: Decodable::decode(buf)?,
                },
                _ => return Err(DecodeError::Custom("unsupported typed transaction type")),
            };
            let (signature, _) = Signature::decode_eip155_inner(buf)?;
            let hash = keccak256(original_encoding).into();
            Ok(TransactionSigned { transaction, hash, signature })
        } else {
            let mut transaction = Transaction::Legacy {
                nonce: Decodable::decode(buf)?,
                gas_price: Decodable::decode(buf)?,
                gas_limit: Decodable::decode(buf)?,
                to: Decodable::decode(buf)?,
                value: Decodable::decode(buf)?,
                input: Bytes(Decodable::decode(buf)?),
                chain_id: None,
            };
            let (signature, extracted_id) = Signature::decode_eip155_inner(buf)?;
            if let Some(id) = extracted_id {
                transaction.set_chain_id(id);
            }
            let hash = keccak256(original_encoding).into();
            Ok(TransactionSigned { transaction, hash, signature })
        }
    }
}

impl TransactionSigned {
    /// Transaction signature.
    pub fn signature(&self) -> &Signature {
        &self.signature
    }

    /// Transaction hash. Used to identify transaction.
    pub fn hash(&self) -> TxHash {
        self.hash
    }

    /// Create a new signed transaction from a transaction and its signature.
    /// This will also calculate the transaction hash using its encoding.
    pub fn from_transaction_and_signature(transaction: Transaction, signature: Signature) -> Self {
        let mut initial_tx = Self { transaction, hash: Default::default(), signature };
        let mut buf = Vec::new();
        initial_tx.encode(&mut buf);
        initial_tx.hash = keccak256(&buf).into();
        initial_tx
    }
}
