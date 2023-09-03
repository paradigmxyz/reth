use super::access_list::AccessList;
use crate::{
    constants::eip4844::DATA_GAS_PER_BLOB,
    keccak256,
    kzg::{
        self, Blob, Bytes48, KzgCommitment, KzgProof, KzgSettings, BYTES_PER_BLOB,
        BYTES_PER_COMMITMENT, BYTES_PER_PROOF,
    },
    kzg_to_versioned_hash, Bytes, ChainId, Signature, Transaction, TransactionKind,
    TransactionSigned, TxHash, TxType, TxValue, B256, EIP4844_TX_TYPE_ID,
};
use alloy_rlp::{length_of_length, Decodable, Encodable, Error as RlpError, Header};
use bytes::BytesMut;
use reth_codecs::{main_codec, Compact};
use serde::{Deserialize, Serialize};
use std::{mem, ops::Deref};

/// [EIP-4844 Blob Transaction](https://eips.ethereum.org/EIPS/eip-4844#blob-transaction)
///
/// A transaction with blob hashes and max blob fee
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct TxEip4844 {
    /// Added as EIP-pub 155: Simple replay attack protection
    pub chain_id: u64,
    /// A scalar value equal to the number of transactions sent by the sender; formally Tn.
    pub nonce: u64,
    /// A scalar value equal to the maximum
    /// amount of gas that should be used in executing
    /// this transaction. This is paid up-front, before any
    /// computation is done and may not be increased
    /// later; formally Tg.
    pub gas_limit: u64,
    /// A scalar value equal to the maximum
    /// amount of gas that should be used in executing
    /// this transaction. This is paid up-front, before any
    /// computation is done and may not be increased
    /// later; formally Tg.
    ///
    /// As ethereum circulation is around 120mil eth as of 2022 that is around
    /// 120000000000000000000000000 wei we are safe to use u128 as its max number is:
    /// 340282366920938463463374607431768211455
    ///
    /// This is also known as `GasFeeCap`
    pub max_fee_per_gas: u128,
    /// Max Priority fee that transaction is paying
    ///
    /// As ethereum circulation is around 120mil eth as of 2022 that is around
    /// 120000000000000000000000000 wei we are safe to use u128 as its max number is:
    /// 340282366920938463463374607431768211455
    ///
    /// This is also known as `GasTipCap`
    pub max_priority_fee_per_gas: u128,
    /// The 160-bit address of the message call’s recipient or, for a contract creation
    /// transaction, ∅, used here to denote the only member of B0 ; formally Tt.
    pub to: TransactionKind,
    /// A scalar value equal to the number of Wei to
    /// be transferred to the message call’s recipient or,
    /// in the case of contract creation, as an endowment
    /// to the newly created account; formally Tv.
    pub value: TxValue,
    /// The accessList specifies a list of addresses and storage keys;
    /// these addresses and storage keys are added into the `accessed_addresses`
    /// and `accessed_storage_keys` global sets (introduced in EIP-2929).
    /// A gas cost is charged, though at a discount relative to the cost of
    /// accessing outside the list.
    pub access_list: AccessList,

    /// It contains a vector of fixed size hash(32 bytes)
    pub blob_versioned_hashes: Vec<B256>,

    /// Max fee per data gas
    ///
    /// aka BlobFeeCap or blobGasFeeCap
    pub max_fee_per_blob_gas: u128,

    /// Input has two uses depending if transaction is Create or Call (if `to` field is None or
    /// Some). pub init: An unlimited size byte array specifying the
    /// EVM-code for the account initialisation procedure CREATE,
    /// data: An unlimited size byte array specifying the
    /// input data of the message call, formally Td.
    pub input: Bytes,
}

impl TxEip4844 {
    /// Returns the effective gas price for the given `base_fee`.
    pub fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        match base_fee {
            None => self.max_fee_per_gas,
            Some(base_fee) => {
                // if the tip is greater than the max priority fee per gas, set it to the max
                // priority fee per gas + base fee
                let tip = self.max_fee_per_gas.saturating_sub(base_fee as u128);
                if tip > self.max_priority_fee_per_gas {
                    self.max_priority_fee_per_gas + base_fee as u128
                } else {
                    // otherwise return the max fee per gas
                    self.max_fee_per_gas
                }
            }
        }
    }

    /// Verifies that the given blob data, commitments, and proofs are all valid for this
    /// transaction.
    ///
    /// Takes as input the [KzgSettings], which should contain the parameters derived from the
    /// KZG trusted setup.
    ///
    /// This ensures that the blob transaction payload has the same number of blob data elements,
    /// commitments, and proofs. Each blob data element is verified against its commitment and
    /// proof.
    ///
    /// Returns [BlobTransactionValidationError::InvalidProof] if any blob KZG proof in the response
    /// fails to verify, or if the versioned hashes in the transaction do not match the actual
    /// commitment versioned hashes.
    pub fn validate_blob(
        &self,
        sidecar: &BlobTransactionSidecar,
        proof_settings: &KzgSettings,
    ) -> Result<(), BlobTransactionValidationError> {
        // Ensure the versioned hashes and commitments have the same length
        if self.blob_versioned_hashes.len() != sidecar.commitments.len() {
            return Err(kzg::Error::MismatchLength(format!(
                "There are {} versioned commitment hashes and {} commitments",
                self.blob_versioned_hashes.len(),
                sidecar.commitments.len()
            ))
            .into())
        }

        // zip and iterate, calculating versioned hashes
        for (versioned_hash, commitment) in
            self.blob_versioned_hashes.iter().zip(sidecar.commitments.iter())
        {
            // convert to KzgCommitment
            let commitment = KzgCommitment::from(*commitment.deref());

            // Calculate the versioned hash
            //
            // TODO: should this method distinguish the type of validation failure? For example
            // whether a certain versioned hash does not match, or whether the blob proof
            // validation failed?
            let calculated_versioned_hash = kzg_to_versioned_hash(commitment);
            if *versioned_hash != calculated_versioned_hash {
                return Err(BlobTransactionValidationError::InvalidProof)
            }
        }

        // Verify as a batch
        let res = KzgProof::verify_blob_kzg_proof_batch(
            sidecar.blobs.as_slice(),
            sidecar.commitments.as_slice(),
            sidecar.proofs.as_slice(),
            proof_settings,
        )
        .map_err(BlobTransactionValidationError::KZGError)?;

        if res {
            Ok(())
        } else {
            Err(BlobTransactionValidationError::InvalidProof)
        }
    }

    /// Returns the total gas for all blobs in this transaction.
    #[inline]
    pub fn blob_gas(&self) -> u64 {
        // SAFETY: we don't expect u64::MAX / DATA_GAS_PER_BLOB hashes in a single transaction
        self.blob_versioned_hashes.len() as u64 * DATA_GAS_PER_BLOB
    }

    /// Decodes the inner [TxEip4844] fields from RLP bytes.
    ///
    /// NOTE: This assumes a RLP header has already been decoded, and _just_ decodes the following
    /// RLP fields in the following order:
    ///
    /// - `chain_id`
    /// - `nonce`
    /// - `max_priority_fee_per_gas`
    /// - `max_fee_per_gas`
    /// - `gas_limit`
    /// - `to`
    /// - `value`
    /// - `data` (`input`)
    /// - `access_list`
    /// - `max_fee_per_blob_gas`
    /// - `blob_versioned_hashes`
    pub fn decode_inner(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self {
            chain_id: Decodable::decode(buf)?,
            nonce: Decodable::decode(buf)?,
            max_priority_fee_per_gas: Decodable::decode(buf)?,
            max_fee_per_gas: Decodable::decode(buf)?,
            gas_limit: Decodable::decode(buf)?,
            to: Decodable::decode(buf)?,
            value: Decodable::decode(buf)?,
            input: Decodable::decode(buf)?,
            access_list: Decodable::decode(buf)?,
            max_fee_per_blob_gas: Decodable::decode(buf)?,
            blob_versioned_hashes: Decodable::decode(buf)?,
        })
    }

    /// Outputs the length of the transaction's fields, without a RLP header.
    pub(crate) fn fields_len(&self) -> usize {
        let mut len = 0;
        len += self.chain_id.length();
        len += self.nonce.length();
        len += self.gas_limit.length();
        len += self.max_fee_per_gas.length();
        len += self.max_priority_fee_per_gas.length();
        len += self.to.length();
        len += self.value.length();
        len += self.access_list.length();
        len += self.blob_versioned_hashes.length();
        len += self.max_fee_per_blob_gas.length();
        len += self.input.0.length();
        len
    }

    /// Encodes only the transaction's fields into the desired buffer, without a RLP header.
    pub(crate) fn encode_fields(&self, out: &mut dyn bytes::BufMut) {
        self.chain_id.encode(out);
        self.nonce.encode(out);
        self.max_priority_fee_per_gas.encode(out);
        self.max_fee_per_gas.encode(out);
        self.gas_limit.encode(out);
        self.to.encode(out);
        self.value.encode(out);
        self.input.0.encode(out);
        self.access_list.encode(out);
        self.max_fee_per_blob_gas.encode(out);
        self.blob_versioned_hashes.encode(out);
    }

    /// Calculates a heuristic for the in-memory size of the [TxEip4844] transaction.
    #[inline]
    pub fn size(&self) -> usize {
        mem::size_of::<ChainId>() + // chain_id
        mem::size_of::<u64>() + // nonce
        mem::size_of::<u64>() + // gas_limit
        mem::size_of::<u128>() + // max_fee_per_gas
        mem::size_of::<u128>() + // max_priority_fee_per_gas
        self.to.size() + // to
        mem::size_of::<TxValue>() + // value
        self.access_list.size() + // access_list
        self.input.len() +  // input
        self.blob_versioned_hashes.capacity() * mem::size_of::<B256>() + // blob hashes size
        mem::size_of::<u128>() // max_fee_per_data_gas
    }

    /// Inner encoding function that is used for both rlp [`Encodable`] trait and for calculating
    /// hash that for eip2718 does not require rlp header
    pub(crate) fn encode_with_signature(
        &self,
        signature: &Signature,
        out: &mut dyn bytes::BufMut,
        with_header: bool,
    ) {
        let payload_length = self.fields_len() + signature.payload_len();
        if with_header {
            Header {
                list: false,
                payload_length: 1 + length_of_length(payload_length) + payload_length,
            }
            .encode(out);
        }
        out.put_u8(self.tx_type() as u8);
        let header = Header { list: true, payload_length };
        header.encode(out);
        self.encode_fields(out);
        signature.encode(out);
    }

    /// Output the length of the RLP signed transaction encoding. This encodes with a RLP header.
    pub(crate) fn payload_len_with_signature(&self, signature: &Signature) -> usize {
        let payload_length = self.fields_len() + signature.payload_len();
        // 'transaction type byte length' + 'header length' + 'payload length'
        let len = 1 + length_of_length(payload_length) + payload_length;
        length_of_length(len) + len
    }

    /// Get transaction type
    pub(crate) fn tx_type(&self) -> TxType {
        TxType::EIP4844
    }

    /// Encodes the legacy transaction in RLP for signing.
    pub(crate) fn encode_for_signing(&self, out: &mut dyn bytes::BufMut) {
        out.put_u8(self.tx_type() as u8);
        Header { list: true, payload_length: self.fields_len() }.encode(out);
        self.encode_fields(out);
    }

    /// Outputs the length of the signature RLP encoding for the transaction.
    pub(crate) fn payload_len_for_signature(&self) -> usize {
        let payload_length = self.fields_len();
        // 'transaction type byte length' + 'header length' + 'payload length'
        1 + length_of_length(payload_length) + payload_length
    }

    /// Outputs the signature hash of the transaction by first encoding without a signature, then
    /// hashing.
    pub(crate) fn signature_hash(&self) -> B256 {
        let mut buf = BytesMut::with_capacity(self.payload_len_for_signature());
        self.encode_for_signing(&mut buf);
        keccak256(&buf)
    }
}

/// An error that can occur when validating a [BlobTransaction].
#[derive(Debug, thiserror::Error)]
pub enum BlobTransactionValidationError {
    /// Proof validation failed.
    #[error("invalid kzg proof")]
    InvalidProof,
    /// An error returned by the [kzg] library
    #[error("kzg error: {0:?}")]
    KZGError(kzg::Error),
    /// The inner transaction is not a blob transaction
    #[error("unable to verify proof for non blob transaction: {0}")]
    NotBlobTransaction(u8),
}

impl From<kzg::Error> for BlobTransactionValidationError {
    fn from(value: kzg::Error) -> Self {
        Self::KZGError(value)
    }
}

/// A response to `GetPooledTransactions` that includes blob data, their commitments, and their
/// corresponding proofs.
///
/// This is defined in [EIP-4844](https://eips.ethereum.org/EIPS/eip-4844#networking) as an element
/// of a `PooledTransactions` response.
///
/// NOTE: This contains a [TransactionSigned], which could be a non-4844 transaction type, even
/// though that would not make sense. This type is meant to be constructed using decoding methods,
/// which should always construct the [TransactionSigned] with an EIP-4844 transaction.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BlobTransaction {
    /// The transaction hash.
    pub hash: TxHash,
    /// The transaction payload.
    pub transaction: TxEip4844,
    /// The transaction signature.
    pub signature: Signature,
    /// The transaction's blob sidecar.
    pub sidecar: BlobTransactionSidecar,
}

impl BlobTransaction {
    /// Constructs a new [BlobTransaction] from a [TransactionSigned] and a
    /// [BlobTransactionSidecar].
    ///
    /// Returns an error if the signed transaction is not [TxEip4844]
    pub fn try_from_signed(
        tx: TransactionSigned,
        sidecar: BlobTransactionSidecar,
    ) -> Result<Self, (TransactionSigned, BlobTransactionSidecar)> {
        let TransactionSigned { transaction, signature, hash } = tx;
        match transaction {
            Transaction::Eip4844(transaction) => Ok(Self { hash, transaction, signature, sidecar }),
            transaction => {
                let tx = TransactionSigned { transaction, signature, hash };
                Err((tx, sidecar))
            }
        }
    }

    /// Verifies that the transaction's blob data, commitments, and proofs are all valid.
    ///
    /// See also [TxEip4844::validate_blob]
    pub fn validate(
        &self,
        proof_settings: &KzgSettings,
    ) -> Result<(), BlobTransactionValidationError> {
        self.transaction.validate_blob(&self.sidecar, proof_settings)
    }

    /// Splits the [BlobTransaction] into its [TransactionSigned] and [BlobTransactionSidecar]
    /// components.
    pub fn into_parts(self) -> (TransactionSigned, BlobTransactionSidecar) {
        let transaction = TransactionSigned {
            transaction: Transaction::Eip4844(self.transaction),
            hash: self.hash,
            signature: self.signature,
        };

        (transaction, self.sidecar)
    }

    /// Encodes the [BlobTransaction] fields as RLP, with a tx type. If `with_header` is `false`,
    /// the following will be encoded:
    /// `tx_type (0x03) || rlp([transaction_payload_body, blobs, commitments, proofs])`
    ///
    /// If `with_header` is `true`, the following will be encoded:
    /// `rlp(tx_type (0x03) || rlp([transaction_payload_body, blobs, commitments, proofs]))`
    ///
    /// NOTE: The header will be a byte string header, not a list header.
    pub(crate) fn encode_with_type_inner(&self, out: &mut dyn bytes::BufMut, with_header: bool) {
        // Calculate the length of:
        // `tx_type || rlp([transaction_payload_body, blobs, commitments, proofs])`
        //
        // to construct and encode the string header
        if with_header {
            Header {
                list: false,
                // add one for the tx type
                payload_length: 1 + self.payload_len(),
            }
            .encode(out);
        }

        out.put_u8(EIP4844_TX_TYPE_ID);

        // Now we encode the inner blob transaction:
        self.encode_inner(out);
    }

    /// Encodes the [BlobTransaction] fields as RLP, with the following format:
    /// `rlp([transaction_payload_body, blobs, commitments, proofs])`
    ///
    /// where `transaction_payload_body` is a list:
    /// `[chain_id, nonce, max_priority_fee_per_gas, ..., y_parity, r, s]`
    ///
    /// Note: this should be used only when implementing other RLP encoding methods, and does not
    /// represent the full RLP encoding of the blob transaction.
    pub(crate) fn encode_inner(&self, out: &mut dyn bytes::BufMut) {
        // First we construct both required list headers.
        //
        // The `transaction_payload_body` length is the length of the fields, plus the length of
        // its list header.
        let tx_header = Header {
            list: true,
            payload_length: self.transaction.fields_len() + self.signature.payload_len(),
        };

        let tx_length = tx_header.length() + tx_header.payload_length;

        // The payload length is the length of the `tranascation_payload_body` list, plus the
        // length of the blobs, commitments, and proofs.
        let payload_length = tx_length + self.sidecar.fields_len();

        // First we use the payload len to construct the first list header
        let blob_tx_header = Header { list: true, payload_length };

        // Encode the blob tx header first
        blob_tx_header.encode(out);

        // Encode the inner tx list header, then its fields
        tx_header.encode(out);
        self.transaction.encode_fields(out);

        // Encode the blobs, commitments, and proofs
        self.sidecar.encode_inner(out);
    }

    /// Ouputs the length of the RLP encoding of the blob transaction, including the tx type byte,
    /// optionally including the length of a wrapping string header. If `with_header` is `false`,
    /// the length of the following will be calculated:
    /// `tx_type (0x03) || rlp([transaction_payload_body, blobs, commitments, proofs])`
    ///
    /// If `with_header` is `true`, the length of the following will be calculated:
    /// `rlp(tx_type (0x03) || rlp([transaction_payload_body, blobs, commitments, proofs]))`
    pub(crate) fn payload_len_with_type(&self, with_header: bool) -> usize {
        if with_header {
            // Construct a header and use that to calculate the total length
            let wrapped_header = Header {
                list: false,
                // add one for the tx type byte
                payload_length: 1 + self.payload_len(),
            };

            // The total length is now the length of the header plus the length of the payload
            // (which includes the tx type byte)
            wrapped_header.length() + wrapped_header.payload_length
        } else {
            // Just add the length of the tx type to the payload length
            1 + self.payload_len()
        }
    }

    /// Outputs the length of the RLP encoding of the blob transaction with the following format:
    /// `rlp([transaction_payload_body, blobs, commitments, proofs])`
    ///
    /// where `transaction_payload_body` is a list:
    /// `[chain_id, nonce, max_priority_fee_per_gas, ..., y_parity, r, s]`
    ///
    /// Note: this should be used only when implementing other RLP encoding length methods, and
    /// does not represent the full RLP encoding of the blob transaction.
    pub(crate) fn payload_len(&self) -> usize {
        // The `transaction_payload_body` length is the length of the fields, plus the length of
        // its list header.
        let tx_header = Header {
            list: true,
            payload_length: self.transaction.fields_len() + self.signature.payload_len(),
        };

        let tx_length = tx_header.length() + tx_header.payload_length;

        // The payload length is the length of the `tranascation_payload_body` list, plus the
        // length of the blobs, commitments, and proofs.
        tx_length + self.sidecar.fields_len()
    }

    /// Decodes a [BlobTransaction] from RLP. This expects the encoding to be:
    /// `rlp([transaction_payload_body, blobs, commitments, proofs])`
    ///
    /// where `transaction_payload_body` is a list:
    /// `[chain_id, nonce, max_priority_fee_per_gas, ..., y_parity, r, s]`
    ///
    /// Note: this should be used only when implementing other RLP decoding methods, and does not
    /// represent the full RLP decoding of the `PooledTransactionsElement` type.
    pub(crate) fn decode_inner(data: &mut &[u8]) -> alloy_rlp::Result<Self> {
        // decode the _first_ list header for the rest of the transaction
        let header = Header::decode(data)?;
        if !header.list {
            return Err(RlpError::Custom("PooledTransactions blob tx must be encoded as a list"))
        }

        // Now we need to decode the inner 4844 transaction and its signature:
        //
        // `[chain_id, nonce, max_priority_fee_per_gas, ..., y_parity, r, s]`
        let header = Header::decode(data)?;
        if !header.list {
            return Err(RlpError::Custom(
                "PooledTransactions inner blob tx must be encoded as a list",
            ))
        }

        // inner transaction
        let transaction = TxEip4844::decode_inner(data)?;

        // signature
        let signature = Signature::decode(data)?;

        // All that's left are the blobs, commitments, and proofs
        let sidecar = BlobTransactionSidecar::decode_inner(data)?;

        // # Calculating the hash
        //
        // The full encoding of the `PooledTransaction` response is:
        // `tx_type (0x03) || rlp([tx_payload_body, blobs, commitments, proofs])`
        //
        // The transaction hash however, is:
        // `keccak256(tx_type (0x03) || rlp(tx_payload_body))`
        //
        // Note that this is `tx_payload_body`, not `[tx_payload_body]`, which would be
        // `[[chain_id, nonce, max_priority_fee_per_gas, ...]]`, i.e. a list within a list.
        //
        // Because the pooled transaction encoding is different than the hash encoding for
        // EIP-4844 transactions, we do not use the original buffer to calculate the hash.
        //
        // Instead, we use `encode_with_signature`, which RLP encodes the transaction with a
        // signature for hashing without a header. We then hash the result.
        let mut buf = Vec::new();
        transaction.encode_with_signature(&signature, &mut buf, false);
        let hash = keccak256(&buf);

        Ok(Self { transaction, hash, signature, sidecar })
    }
}

/// This represents a set of blobs, and its corresponding commitments and proofs.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(C)]
pub struct BlobTransactionSidecar {
    /// The blob data.
    pub blobs: Vec<Blob>,
    /// The blob commitments.
    pub commitments: Vec<Bytes48>,
    /// The blob proofs.
    pub proofs: Vec<Bytes48>,
}

impl BlobTransactionSidecar {
    /// Encodes the inner [BlobTransactionSidecar] fields as RLP bytes, without a RLP header.
    ///
    /// This encodes the fields in the following order:
    /// - `blobs`
    /// - `commitments`
    /// - `proofs`
    pub(crate) fn encode_inner(&self, out: &mut dyn bytes::BufMut) {
        BlobTransactionSidecarRlp::wrap_ref(self).encode(out);
    }

    /// Outputs the RLP length of the [BlobTransactionSidecar] fields, without a RLP header.
    pub(crate) fn fields_len(&self) -> usize {
        self.blobs.len() + self.commitments.len() + self.proofs.len()
    }

    /// Decodes the inner [BlobTransactionSidecar] fields from RLP bytes, without a RLP header.
    ///
    /// This decodes the fields in the following order:
    /// - `blobs`
    /// - `commitments`
    /// - `proofs`
    pub(crate) fn decode_inner(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(BlobTransactionSidecarRlp::decode(buf)?.unwrap())
    }

    /// Calculates a size heuristic for the in-memory size of the [BlobTransactionSidecar].
    #[inline]
    pub fn size(&self) -> usize {
        self.blobs.len() * BYTES_PER_BLOB + // blobs
        self.commitments.len() * BYTES_PER_COMMITMENT + // commitments
        self.proofs.len() * BYTES_PER_PROOF // proofs
    }
}

// Wrapper for c-kzg rlp
#[repr(C)]
struct BlobTransactionSidecarRlp {
    blobs: Vec<[u8; c_kzg::BYTES_PER_BLOB]>,
    commitments: Vec<[u8; 48]>,
    proofs: Vec<[u8; 48]>,
}

const _: [(); std::mem::size_of::<BlobTransactionSidecar>()] =
    [(); std::mem::size_of::<BlobTransactionSidecarRlp>()];

impl BlobTransactionSidecarRlp {
    fn wrap_ref(other: &BlobTransactionSidecar) -> &Self {
        // SAFETY: Same repr and size
        unsafe { &*(other as *const BlobTransactionSidecar).cast::<Self>() }
    }

    fn unwrap(self) -> BlobTransactionSidecar {
        // SAFETY: Same repr and size
        unsafe { std::mem::transmute(self) }
    }

    fn encode(&self, out: &mut dyn bytes::BufMut) {
        // Encode the blobs, commitments, and proofs
        self.blobs.encode(out);
        self.commitments.encode(out);
        self.proofs.encode(out);
    }

    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self {
            blobs: Decodable::decode(buf)?,
            commitments: Decodable::decode(buf)?,
            proofs: Decodable::decode(buf)?,
        })
    }
}
