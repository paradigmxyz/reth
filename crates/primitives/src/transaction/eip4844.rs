use super::access_list::AccessList;
use crate::{
    constants::eip4844::DATA_GAS_PER_BLOB, keccak256, Address, Bytes, ChainId, Signature, TxType,
    B256, U256,
};
use alloy_rlp::{length_of_length, Decodable, Encodable, Header};
use reth_codecs::{main_codec, Compact, CompactPlaceholder};
use std::mem;

#[cfg(feature = "c-kzg")]
use crate::kzg::KzgSettings;

/// [EIP-4844 Blob Transaction](https://eips.ethereum.org/EIPS/eip-4844#blob-transaction)
///
/// A transaction with blob hashes and max blob fee
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct TxEip4844 {
    /// Added as EIP-pub 155: Simple replay attack protection
    pub chain_id: ChainId,
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
    /// TODO(debt): this should be removed if we break the DB.
    /// Makes sure that the Compact bitflag struct has one bit after the above field:
    /// <https://github.com/paradigmxyz/reth/pull/8291#issuecomment-2117545016>
    pub placeholder: Option<CompactPlaceholder>,
    /// The 160-bit address of the message call’s recipient.
    pub to: Address,
    /// A scalar value equal to the number of Wei to
    /// be transferred to the message call’s recipient or,
    /// in the case of contract creation, as an endowment
    /// to the newly created account; formally Tv.
    pub value: U256,
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
    pub const fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
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
    /// Returns `InvalidProof` if any blob KZG proof in the response
    /// fails to verify, or if the versioned hashes in the transaction do not match the actual
    /// commitment versioned hashes.
    #[cfg(feature = "c-kzg")]
    pub fn validate_blob(
        &self,
        sidecar: &crate::BlobTransactionSidecar,
        proof_settings: &KzgSettings,
    ) -> Result<(), alloy_eips::eip4844::BlobTransactionValidationError> {
        sidecar.validate(&self.blob_versioned_hashes, proof_settings)
    }

    /// Returns the total gas for all blobs in this transaction.
    #[inline]
    pub fn blob_gas(&self) -> u64 {
        // NOTE: we don't expect u64::MAX / DATA_GAS_PER_BLOB hashes in a single transaction
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
            placeholder: Some(()),
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
        self.chain_id.length() +
            self.nonce.length() +
            self.gas_limit.length() +
            self.max_fee_per_gas.length() +
            self.max_priority_fee_per_gas.length() +
            self.to.length() +
            self.value.length() +
            self.access_list.length() +
            self.blob_versioned_hashes.length() +
            self.max_fee_per_blob_gas.length() +
            self.input.0.length()
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
        mem::size_of::<Address>() + // to
        mem::size_of::<U256>() + // value
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
        let len = self.payload_len_with_signature_without_header(signature);
        length_of_length(len) + len
    }

    /// Output the length of the RLP signed transaction encoding, _without_ a RLP header.
    pub(crate) fn payload_len_with_signature_without_header(&self, signature: &Signature) -> usize {
        let payload_length = self.fields_len() + signature.payload_len();
        // 'transaction type byte length' + 'header length' + 'payload length'
        1 + length_of_length(payload_length) + payload_length
    }

    /// Get transaction type
    pub(crate) const fn tx_type(&self) -> TxType {
        TxType::Eip4844
    }

    /// Encodes the EIP-4844 transaction in RLP for signing.
    ///
    /// This encodes the transaction as:
    /// `tx_type || rlp(chain_id, nonce, max_priority_fee_per_gas, max_fee_per_gas, gas_limit, to,
    /// value, input, access_list, max_fee_per_blob_gas, blob_versioned_hashes)`
    ///
    /// Note that there is no rlp header before the transaction type byte.
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
        let mut buf = Vec::with_capacity(self.payload_len_for_signature());
        self.encode_for_signing(&mut buf);
        keccak256(&buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, bytes};

    #[test]
    fn backwards_compatible_txkind_test() {
        // TxEip4844 encoded with TxKind on to field
        // holesky tx hash: <0xa3b1668225bf0fbfdd6c19aa6fd071fa4ff5d09a607c67ccd458b97735f745ac>
        let tx = bytes!("224348a100426844cb2dc6c0b2d05e003b9aca0079c9109b764609df928d16fc4a91e9081f7e87db09310001019101fb28118ceccaabca22a47e35b9c3f12eb2dcb25e5c543d5b75e6cd841f0a05328d26ef16e8450000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000052000000000000000000000000000000000000000000000000000000000000004c000000000000000000000000000000000000000000000000000000000000000200000000000000000000000007b399987d24fc5951f3e94a4cb16e87414bf22290000000000000000000000001670090000000000000000000000000000010001302e31382e302d64657600000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000c00000000000000000000000000000000000000000000000000000000000000420000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000200000000000000000000000009e640a6aadf4f664cf467b795c31332f44acbe6c000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000002c00000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006614c2d1000000000000000000000000000000000000000000000000000000000014012c000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000001e000000000000000000000000000000000000000000000000000000000000000030000000000000000000000000000000000000000000000000000000000000064000000000000000000000000000000000000000000000000000000000000093100000000000000000000000000000000000000000000000000000000000000c8000000000000000000000000000000000000000000000000000000000000093100000000000000000000000000000000000000000000000000000000000003e800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000041f06fd78f4dcdf089263524731620941747b9b93fd8f631557e25b23845a78b685bd82f9d36bce2f4cc812b6e5191df52479d349089461ffe76e9f2fa2848a0fe1b0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000410819f04aba17677807c61ae72afdddf7737f26931ecfa8af05b7c669808b36a2587e32c90bb0ed2100266dd7797c80121a109a2b0fe941ca5a580e438988cac81c000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000");
        let (tx, _) = TxEip4844::from_compact(&tx, tx.len());
        assert_eq!(tx.to, address!("79C9109b764609df928d16fC4a91e9081F7e87DB"));
        assert_eq!(tx.placeholder, Some(()));
        assert_eq!(tx.input, bytes!("ef16e8450000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000052000000000000000000000000000000000000000000000000000000000000004c000000000000000000000000000000000000000000000000000000000000000200000000000000000000000007b399987d24fc5951f3e94a4cb16e87414bf22290000000000000000000000001670090000000000000000000000000000010001302e31382e302d64657600000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000c00000000000000000000000000000000000000000000000000000000000000420000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000200000000000000000000000009e640a6aadf4f664cf467b795c31332f44acbe6c000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000002c00000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006614c2d1000000000000000000000000000000000000000000000000000000000014012c000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000001e000000000000000000000000000000000000000000000000000000000000000030000000000000000000000000000000000000000000000000000000000000064000000000000000000000000000000000000000000000000000000000000093100000000000000000000000000000000000000000000000000000000000000c8000000000000000000000000000000000000000000000000000000000000093100000000000000000000000000000000000000000000000000000000000003e800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000041f06fd78f4dcdf089263524731620941747b9b93fd8f631557e25b23845a78b685bd82f9d36bce2f4cc812b6e5191df52479d349089461ffe76e9f2fa2848a0fe1b0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000410819f04aba17677807c61ae72afdddf7737f26931ecfa8af05b7c669808b36a2587e32c90bb0ed2100266dd7797c80121a109a2b0fe941ca5a580e438988cac81c000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"));
    }
}
