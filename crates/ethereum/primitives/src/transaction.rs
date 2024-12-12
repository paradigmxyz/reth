use alloc::vec::Vec;
use alloy_consensus::{
    transaction::RlpEcdsaTx, SignableTransaction, Signed, TxEip1559, TxEip2930, TxEip4844,
    TxEip7702, TxLegacy, TxType, Typed2718,
};
use alloy_eips::{
    eip2718::{Decodable2718, Eip2718Error, Eip2718Result, Encodable2718},
    eip2930::AccessList,
    eip7702::SignedAuthorization,
};
use alloy_primitives::{
    keccak256, Address, Bytes, ChainId, PrimitiveSignature as Signature, TxHash, TxKind, B256, U256,
};
use alloy_rlp::{Decodable, Encodable};
use core::hash::{Hash, Hasher};
use once_cell as _;
#[cfg(not(feature = "std"))]
use once_cell::sync::OnceCell as OnceLock;
use reth_primitives_traits::{
    crypto::secp256k1::{recover_signer, recover_signer_unchecked},
    InMemorySize, SignedTransaction,
};
use serde::{Deserialize, Serialize};
#[cfg(feature = "std")]
use std::sync::OnceLock;

macro_rules! delegate {
    ($self:expr => $tx:ident.$method:ident($($arg:expr),*)) => {
        match $self {
            Transaction::Legacy($tx) => $tx.$method($($arg),*),
            Transaction::Eip2930($tx) => $tx.$method($($arg),*),
            Transaction::Eip1559($tx) => $tx.$method($($arg),*),
            Transaction::Eip4844($tx) => $tx.$method($($arg),*),
            Transaction::Eip7702($tx) => $tx.$method($($arg),*),
        }
    };
}

/// A raw transaction.
///
/// Transaction types were introduced in [EIP-2718](https://eips.ethereum.org/EIPS/eip-2718).
#[derive(Debug, Clone, PartialEq, Eq, Hash, derive_more::From, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
pub enum Transaction {
    /// Legacy transaction (type `0x0`).
    ///
    /// Traditional Ethereum transactions, containing parameters `nonce`, `gasPrice`, `gasLimit`,
    /// `to`, `value`, `data`, `v`, `r`, and `s`.
    ///
    /// These transactions do not utilize access lists nor do they incorporate EIP-1559 fee market
    /// changes.
    Legacy(TxLegacy),
    /// Transaction with an [`AccessList`] ([EIP-2930](https://eips.ethereum.org/EIPS/eip-2930)), type `0x1`.
    ///
    /// The `accessList` specifies an array of addresses and storage keys that the transaction
    /// plans to access, enabling gas savings on cross-contract calls by pre-declaring the accessed
    /// contract and storage slots.
    Eip2930(TxEip2930),
    /// A transaction with a priority fee ([EIP-1559](https://eips.ethereum.org/EIPS/eip-1559)), type `0x2`.
    ///
    /// Unlike traditional transactions, EIP-1559 transactions use an in-protocol, dynamically
    /// changing base fee per gas, adjusted at each block to manage network congestion.
    ///
    /// - `maxPriorityFeePerGas`, specifying the maximum fee above the base fee the sender is
    ///   willing to pay
    /// - `maxFeePerGas`, setting the maximum total fee the sender is willing to pay.
    ///
    /// The base fee is burned, while the priority fee is paid to the miner who includes the
    /// transaction, incentivizing miners to include transactions with higher priority fees per
    /// gas.
    Eip1559(TxEip1559),
    /// Shard Blob Transactions ([EIP-4844](https://eips.ethereum.org/EIPS/eip-4844)), type `0x3`.
    ///
    /// Shard Blob Transactions introduce a new transaction type called a blob-carrying transaction
    /// to reduce gas costs. These transactions are similar to regular Ethereum transactions but
    /// include additional data called a blob.
    ///
    /// Blobs are larger (~125 kB) and cheaper than the current calldata, providing an immutable
    /// and read-only memory for storing transaction data.
    ///
    /// EIP-4844, also known as proto-danksharding, implements the framework and logic of
    /// danksharding, introducing new transaction formats and verification rules.
    Eip4844(TxEip4844),
    /// EOA Set Code Transactions ([EIP-7702](https://eips.ethereum.org/EIPS/eip-7702)), type `0x4`.
    ///
    /// EOA Set Code Transactions give the ability to temporarily set contract code for an
    /// EOA for a single transaction. This allows for temporarily adding smart contract
    /// functionality to the EOA.
    Eip7702(TxEip7702),
}

impl Transaction {
    /// Returns [`TxType`] of the transaction.
    pub const fn tx_type(&self) -> TxType {
        match self {
            Self::Legacy(_) => TxType::Legacy,
            Self::Eip2930(_) => TxType::Eip2930,
            Self::Eip1559(_) => TxType::Eip1559,
            Self::Eip4844(_) => TxType::Eip4844,
            Self::Eip7702(_) => TxType::Eip7702,
        }
    }
}

impl Typed2718 for Transaction {
    fn ty(&self) -> u8 {
        delegate!(self => tx.ty())
    }
}

impl alloy_consensus::Transaction for Transaction {
    fn chain_id(&self) -> Option<ChainId> {
        delegate!(self => tx.chain_id())
    }

    fn nonce(&self) -> u64 {
        delegate!(self => tx.nonce())
    }

    fn gas_limit(&self) -> u64 {
        delegate!(self => tx.gas_limit())
    }

    fn gas_price(&self) -> Option<u128> {
        delegate!(self => tx.gas_price())
    }

    fn max_fee_per_gas(&self) -> u128 {
        delegate!(self => tx.max_fee_per_gas())
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        delegate!(self => tx.max_priority_fee_per_gas())
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        delegate!(self => tx.max_fee_per_blob_gas())
    }

    fn priority_fee_or_price(&self) -> u128 {
        delegate!(self => tx.priority_fee_or_price())
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        delegate!(self => tx.effective_gas_price(base_fee))
    }

    fn is_dynamic_fee(&self) -> bool {
        delegate!(self => tx.is_dynamic_fee())
    }

    fn kind(&self) -> alloy_primitives::TxKind {
        delegate!(self => tx.kind())
    }

    fn access_list(&self) -> Option<&alloy_eips::eip2930::AccessList> {
        delegate!(self => tx.access_list())
    }

    fn authorization_list(&self) -> Option<&[alloy_eips::eip7702::SignedAuthorization]> {
        delegate!(self => tx.authorization_list())
    }

    fn is_create(&self) -> bool {
        delegate!(self => tx.is_create())
    }

    fn value(&self) -> alloy_primitives::U256 {
        delegate!(self => tx.value())
    }

    fn input(&self) -> &alloy_primitives::Bytes {
        delegate!(self => tx.input())
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        delegate!(self => tx.blob_versioned_hashes())
    }
}

impl SignableTransaction<Signature> for Transaction {
    fn set_chain_id(&mut self, chain_id: alloy_primitives::ChainId) {
        delegate!(self => tx.set_chain_id(chain_id))
    }

    fn encode_for_signing(&self, out: &mut dyn alloy_rlp::BufMut) {
        delegate!(self => tx.encode_for_signing(out))
    }

    fn payload_len_for_signature(&self) -> usize {
        delegate!(self => tx.payload_len_for_signature())
    }

    fn into_signed(self, signature: Signature) -> Signed<Self> {
        let tx_hash = delegate!(&self => tx.tx_hash(&signature));
        Signed::new_unchecked(self, signature, tx_hash)
    }
}

impl InMemorySize for Transaction {
    fn size(&self) -> usize {
        delegate!(self => tx.size())
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for Transaction {
    // Serializes the TxType to the buffer if necessary, returning 2 bits of the type as an
    // identifier instead of the length.
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: alloy_rlp::bytes::BufMut + AsMut<[u8]>,
    {
        let identifier = self.tx_type().to_compact(buf);
        delegate!(self => tx.to_compact(buf));
        identifier
    }

    // For backwards compatibility purposes, only 2 bits of the type are encoded in the identifier
    // parameter. In the case of a [`COMPACT_EXTENDED_IDENTIFIER_FLAG`], the full transaction type
    // is read from the buffer as a single byte.
    //
    // # Panics
    //
    // A panic will be triggered if an identifier larger than 3 is passed from the database. For
    // optimism a identifier with value [`DEPOSIT_TX_TYPE_ID`] is allowed.
    fn from_compact(buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        let (tx_type, buf) = TxType::from_compact(buf, identifier);

        match tx_type {
            TxType::Legacy => {
                let (tx, buf) = TxLegacy::from_compact(buf, buf.len());
                (Self::Legacy(tx), buf)
            }
            TxType::Eip2930 => {
                let (tx, buf) = TxEip2930::from_compact(buf, buf.len());
                (Self::Eip2930(tx), buf)
            }
            TxType::Eip1559 => {
                let (tx, buf) = TxEip1559::from_compact(buf, buf.len());
                (Self::Eip1559(tx), buf)
            }
            TxType::Eip4844 => {
                let (tx, buf) = TxEip4844::from_compact(buf, buf.len());
                (Self::Eip4844(tx), buf)
            }
            TxType::Eip7702 => {
                let (tx, buf) = TxEip7702::from_compact(buf, buf.len());
                (Self::Eip7702(tx), buf)
            }
        }
    }
}

/// Signed Ethereum transaction.
#[derive(Debug, Clone, Eq, Serialize, Deserialize, derive_more::AsRef, derive_more::Deref)]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(rlp))]
#[serde(rename_all = "camelCase")]
pub struct TransactionSigned {
    /// Transaction hash
    #[serde(skip)]
    pub hash: OnceLock<TxHash>,
    /// The transaction signature values
    pub signature: Signature,
    /// Raw transaction info
    #[deref]
    #[as_ref]
    pub transaction: Transaction,
}

impl TransactionSigned {
    fn recalculate_hash(&self) -> B256 {
        keccak256(self.encoded_2718())
    }
}

impl Hash for TransactionSigned {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.signature.hash(state);
        self.transaction.hash(state);
    }
}

impl PartialEq for TransactionSigned {
    fn eq(&self, other: &Self) -> bool {
        self.signature == other.signature &&
            self.transaction == other.transaction &&
            self.tx_hash() == other.tx_hash()
    }
}

impl Typed2718 for TransactionSigned {
    fn ty(&self) -> u8 {
        self.transaction.ty()
    }
}

impl alloy_consensus::Transaction for TransactionSigned {
    fn chain_id(&self) -> Option<ChainId> {
        self.transaction.chain_id()
    }

    fn nonce(&self) -> u64 {
        self.transaction.nonce()
    }

    fn gas_limit(&self) -> u64 {
        self.transaction.gas_limit()
    }

    fn gas_price(&self) -> Option<u128> {
        self.transaction.gas_price()
    }

    fn max_fee_per_gas(&self) -> u128 {
        self.transaction.max_fee_per_gas()
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.transaction.max_priority_fee_per_gas()
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        self.transaction.max_fee_per_blob_gas()
    }

    fn priority_fee_or_price(&self) -> u128 {
        self.transaction.priority_fee_or_price()
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        self.transaction.effective_gas_price(base_fee)
    }

    fn is_dynamic_fee(&self) -> bool {
        self.transaction.is_dynamic_fee()
    }

    fn kind(&self) -> TxKind {
        self.transaction.kind()
    }

    fn is_create(&self) -> bool {
        self.transaction.is_create()
    }

    fn value(&self) -> U256 {
        self.transaction.value()
    }

    fn input(&self) -> &Bytes {
        self.transaction.input()
    }

    fn access_list(&self) -> Option<&AccessList> {
        self.transaction.access_list()
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        self.transaction.blob_versioned_hashes()
    }

    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        self.transaction.authorization_list()
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for TransactionSigned {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        #[allow(unused_mut)]
        let mut transaction = Transaction::arbitrary(u)?;

        let secp = secp256k1::Secp256k1::new();
        let key_pair = secp256k1::Keypair::new(&secp, &mut rand::thread_rng());
        let signature = reth_primitives_traits::crypto::secp256k1::sign_message(
            B256::from_slice(&key_pair.secret_bytes()[..]),
            transaction.signature_hash(),
        )
        .unwrap();

        Ok(Self { transaction, signature, hash: Default::default() })
    }
}

impl InMemorySize for TransactionSigned {
    fn size(&self) -> usize {
        let Self { hash: _, signature, transaction } = self;
        self.tx_hash().size() + signature.size() + transaction.size()
    }
}

impl Encodable2718 for TransactionSigned {
    fn type_flag(&self) -> Option<u8> {
        (!self.transaction.is_legacy()).then(|| self.ty())
    }

    fn encode_2718_len(&self) -> usize {
        delegate!(&self.transaction => tx.eip2718_encoded_length(&self.signature))
    }

    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        delegate!(&self.transaction => tx.eip2718_encode(&self.signature, out))
    }

    fn trie_hash(&self) -> B256 {
        *self.tx_hash()
    }
}

impl Decodable2718 for TransactionSigned {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        match ty.try_into().map_err(|_| Eip2718Error::UnexpectedType(ty))? {
            TxType::Legacy => Err(Eip2718Error::UnexpectedType(0)),
            TxType::Eip2930 => {
                let (tx, signature) = TxEip2930::rlp_decode_with_signature(buf)?;
                Ok(Self {
                    transaction: Transaction::Eip2930(tx),
                    signature,
                    hash: Default::default(),
                })
            }
            TxType::Eip1559 => {
                let (tx, signature) = TxEip1559::rlp_decode_with_signature(buf)?;
                Ok(Self {
                    transaction: Transaction::Eip1559(tx),
                    signature,
                    hash: Default::default(),
                })
            }
            TxType::Eip4844 => {
                let (tx, signature) = TxEip4844::rlp_decode_with_signature(buf)?;
                Ok(Self {
                    transaction: Transaction::Eip4844(tx),
                    signature,
                    hash: Default::default(),
                })
            }
            TxType::Eip7702 => {
                let (tx, signature) = TxEip7702::rlp_decode_with_signature(buf)?;
                Ok(Self {
                    transaction: Transaction::Eip7702(tx),
                    signature,
                    hash: Default::default(),
                })
            }
        }
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        let (tx, signature) = TxLegacy::rlp_decode_with_signature(buf)?;
        Ok(Self { transaction: Transaction::Legacy(tx), signature, hash: Default::default() })
    }
}

impl Encodable for TransactionSigned {
    fn length(&self) -> usize {
        self.network_len()
    }

    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.network_encode(out);
    }
}

impl Decodable for TransactionSigned {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Self::network_decode(buf).map_err(Into::into)
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for TransactionSigned {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: alloy_rlp::bytes::BufMut + AsMut<[u8]>,
    {
        use alloy_consensus::Transaction;

        let start = buf.as_mut().len();

        // Placeholder for bitflags.
        // The first byte uses 4 bits as flags: IsCompressed[1bit], TxType[2bits], Signature[1bit]
        buf.put_u8(0);

        let sig_bit = self.signature.to_compact(buf) as u8;
        let zstd_bit = self.transaction.input().len() >= 32;

        let tx_bits = if zstd_bit {
            let mut tmp = Vec::with_capacity(256);
            if cfg!(feature = "std") {
                reth_zstd_compressors::TRANSACTION_COMPRESSOR.with(|compressor| {
                    let mut compressor = compressor.borrow_mut();
                    let tx_bits = self.transaction.to_compact(&mut tmp);
                    buf.put_slice(&compressor.compress(&tmp).expect("Failed to compress"));
                    tx_bits as u8
                })
            } else {
                let mut compressor = reth_zstd_compressors::create_tx_compressor();
                let tx_bits = self.transaction.to_compact(&mut tmp);
                buf.put_slice(&compressor.compress(&tmp).expect("Failed to compress"));
                tx_bits as u8
            }
        } else {
            self.transaction.to_compact(buf) as u8
        };

        // Replace bitflags with the actual values
        buf.as_mut()[start] = sig_bit | (tx_bits << 1) | ((zstd_bit as u8) << 3);

        buf.as_mut().len() - start
    }

    fn from_compact(mut buf: &[u8], _len: usize) -> (Self, &[u8]) {
        use alloy_rlp::bytes::Buf;

        // The first byte uses 4 bits as flags: IsCompressed[1], TxType[2], Signature[1]
        let bitflags = buf.get_u8() as usize;

        let sig_bit = bitflags & 1;
        let (signature, buf) = Signature::from_compact(buf, sig_bit);

        let zstd_bit = bitflags >> 3;
        let (transaction, buf) = if zstd_bit != 0 {
            if cfg!(feature = "std") {
                reth_zstd_compressors::TRANSACTION_DECOMPRESSOR.with(|decompressor| {
                    let mut decompressor = decompressor.borrow_mut();

                    // TODO: enforce that zstd is only present at a "top" level type

                    let transaction_type = (bitflags & 0b110) >> 1;
                    let (transaction, _) =
                        Transaction::from_compact(decompressor.decompress(buf), transaction_type);

                    (transaction, buf)
                })
            } else {
                let mut decompressor = reth_zstd_compressors::create_tx_decompressor();
                let transaction_type = (bitflags & 0b110) >> 1;
                let (transaction, _) =
                    Transaction::from_compact(decompressor.decompress(buf), transaction_type);

                (transaction, buf)
            }
        } else {
            let transaction_type = bitflags >> 1;
            Transaction::from_compact(buf, transaction_type)
        };

        (Self { signature, transaction, hash: Default::default() }, buf)
    }
}

impl SignedTransaction for TransactionSigned {
    fn tx_hash(&self) -> &TxHash {
        self.hash.get_or_init(|| self.recalculate_hash())
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn recover_signer(&self) -> Option<Address> {
        let signature_hash = self.signature_hash();
        recover_signer(&self.signature, signature_hash)
    }

    fn recover_signer_unchecked_with_buf(&self, buf: &mut Vec<u8>) -> Option<Address> {
        self.encode_for_signing(buf);
        let signature_hash = keccak256(buf);
        recover_signer_unchecked(&self.signature, signature_hash)
    }
}
