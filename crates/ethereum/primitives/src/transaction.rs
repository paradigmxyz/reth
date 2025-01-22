use alloc::vec::Vec;
pub use alloy_consensus::{transaction::PooledTransaction, TxType};
use alloy_consensus::{
    transaction::RlpEcdsaTx, BlobTransactionSidecar, SignableTransaction, Signed, TxEip1559,
    TxEip2930, TxEip4844, TxEip4844WithSidecar, TxEip7702, TxLegacy, Typed2718, TypedTransaction,
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
    transaction::{error::TransactionConversionError, signed::RecoveryError},
    FillTxEnv, InMemorySize, SignedTransaction,
};
use revm_primitives::{AuthorizationList, TxEnv};
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

    /// This sets the transaction's nonce.
    pub fn set_nonce(&mut self, nonce: u64) {
        match self {
            Self::Legacy(tx) => tx.nonce = nonce,
            Self::Eip2930(tx) => tx.nonce = nonce,
            Self::Eip1559(tx) => tx.nonce = nonce,
            Self::Eip4844(tx) => tx.nonce = nonce,
            Self::Eip7702(tx) => tx.nonce = nonce,
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

impl From<TypedTransaction> for Transaction {
    fn from(value: TypedTransaction) -> Self {
        match value {
            TypedTransaction::Legacy(tx) => Self::Legacy(tx),
            TypedTransaction::Eip2930(tx) => Self::Eip2930(tx),
            TypedTransaction::Eip1559(tx) => Self::Eip1559(tx),
            TypedTransaction::Eip4844(tx) => Self::Eip4844(tx.into()),
            TypedTransaction::Eip7702(tx) => Self::Eip7702(tx),
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

impl Default for TransactionSigned {
    fn default() -> Self {
        Self::new_unhashed(Transaction::Legacy(Default::default()), Signature::test_signature())
    }
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

impl TransactionSigned {
    /// Creates a new signed transaction from the given transaction, signature and hash.
    pub fn new(transaction: Transaction, signature: Signature, hash: B256) -> Self {
        Self { hash: hash.into(), signature, transaction }
    }

    /// Creates a new signed transaction from the given transaction and signature without the hash.
    ///
    /// Note: this only calculates the hash on the first [`TransactionSigned::hash`] call.
    pub fn new_unhashed(transaction: Transaction, signature: Signature) -> Self {
        Self { hash: Default::default(), signature, transaction }
    }

    /// Converts from an EIP-4844 transaction to a [`PooledTransaction`] with the given sidecar.
    ///
    /// Returns an `Err` containing the original `TransactionSigned` if the transaction is not
    /// EIP-4844.
    pub fn try_into_pooled_eip4844(
        self,
        sidecar: BlobTransactionSidecar,
    ) -> Result<PooledTransaction, Self> {
        let hash = *self.tx_hash();
        Ok(match self {
            // If the transaction is an EIP-4844 transaction...
            Self { transaction: Transaction::Eip4844(tx), signature, .. } => {
                // Construct a pooled eip488 tx with the provided sidecar.
                PooledTransaction::Eip4844(Signed::new_unchecked(
                    TxEip4844WithSidecar { tx, sidecar },
                    signature,
                    hash,
                ))
            }
            // If the transaction is not EIP-4844, return an error with the original
            // transaction.
            _ => return Err(self),
        })
    }

    /// Returns the [`TxEip4844`] if the transaction is an EIP-4844 transaction.
    pub const fn as_eip4844(&self) -> Option<&TxEip4844> {
        match &self.transaction {
            Transaction::Eip4844(tx) => Some(tx),
            _ => None,
        }
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

impl FillTxEnv for TransactionSigned {
    fn fill_tx_env(&self, tx_env: &mut TxEnv, sender: Address) {
        tx_env.caller = sender;
        match self.as_ref() {
            Transaction::Legacy(tx) => {
                tx_env.gas_limit = tx.gas_limit;
                tx_env.gas_price = U256::from(tx.gas_price);
                tx_env.gas_priority_fee = None;
                tx_env.transact_to = tx.to;
                tx_env.value = tx.value;
                tx_env.data = tx.input.clone();
                tx_env.chain_id = tx.chain_id;
                tx_env.nonce = Some(tx.nonce);
                tx_env.access_list.clear();
                tx_env.blob_hashes.clear();
                tx_env.max_fee_per_blob_gas.take();
                tx_env.authorization_list = None;
            }
            Transaction::Eip2930(tx) => {
                tx_env.gas_limit = tx.gas_limit;
                tx_env.gas_price = U256::from(tx.gas_price);
                tx_env.gas_priority_fee = None;
                tx_env.transact_to = tx.to;
                tx_env.value = tx.value;
                tx_env.data = tx.input.clone();
                tx_env.chain_id = Some(tx.chain_id);
                tx_env.nonce = Some(tx.nonce);
                tx_env.access_list.clone_from(&tx.access_list.0);
                tx_env.blob_hashes.clear();
                tx_env.max_fee_per_blob_gas.take();
                tx_env.authorization_list = None;
            }
            Transaction::Eip1559(tx) => {
                tx_env.gas_limit = tx.gas_limit;
                tx_env.gas_price = U256::from(tx.max_fee_per_gas);
                tx_env.gas_priority_fee = Some(U256::from(tx.max_priority_fee_per_gas));
                tx_env.transact_to = tx.to;
                tx_env.value = tx.value;
                tx_env.data = tx.input.clone();
                tx_env.chain_id = Some(tx.chain_id);
                tx_env.nonce = Some(tx.nonce);
                tx_env.access_list.clone_from(&tx.access_list.0);
                tx_env.blob_hashes.clear();
                tx_env.max_fee_per_blob_gas.take();
                tx_env.authorization_list = None;
            }
            Transaction::Eip4844(tx) => {
                tx_env.gas_limit = tx.gas_limit;
                tx_env.gas_price = U256::from(tx.max_fee_per_gas);
                tx_env.gas_priority_fee = Some(U256::from(tx.max_priority_fee_per_gas));
                tx_env.transact_to = TxKind::Call(tx.to);
                tx_env.value = tx.value;
                tx_env.data = tx.input.clone();
                tx_env.chain_id = Some(tx.chain_id);
                tx_env.nonce = Some(tx.nonce);
                tx_env.access_list.clone_from(&tx.access_list.0);
                tx_env.blob_hashes.clone_from(&tx.blob_versioned_hashes);
                tx_env.max_fee_per_blob_gas = Some(U256::from(tx.max_fee_per_blob_gas));
                tx_env.authorization_list = None;
            }
            Transaction::Eip7702(tx) => {
                tx_env.gas_limit = tx.gas_limit;
                tx_env.gas_price = U256::from(tx.max_fee_per_gas);
                tx_env.gas_priority_fee = Some(U256::from(tx.max_priority_fee_per_gas));
                tx_env.transact_to = tx.to.into();
                tx_env.value = tx.value;
                tx_env.data = tx.input.clone();
                tx_env.chain_id = Some(tx.chain_id);
                tx_env.nonce = Some(tx.nonce);
                tx_env.access_list.clone_from(&tx.access_list.0);
                tx_env.blob_hashes.clear();
                tx_env.max_fee_per_blob_gas.take();
                tx_env.authorization_list =
                    Some(AuthorizationList::Signed(tx.authorization_list.clone()));
            }
        }
    }
}

impl SignedTransaction for TransactionSigned {
    fn tx_hash(&self) -> &TxHash {
        self.hash.get_or_init(|| self.recalculate_hash())
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn recover_signer(&self) -> Result<Address, RecoveryError> {
        let signature_hash = self.signature_hash();
        recover_signer(&self.signature, signature_hash)
    }

    fn recover_signer_unchecked_with_buf(
        &self,
        buf: &mut Vec<u8>,
    ) -> Result<Address, RecoveryError> {
        self.encode_for_signing(buf);
        let signature_hash = keccak256(buf);
        recover_signer_unchecked(&self.signature, signature_hash)
    }
}

impl TryFrom<TransactionSigned> for PooledTransaction {
    type Error = TransactionConversionError;

    fn try_from(tx: TransactionSigned) -> Result<Self, Self::Error> {
        let hash = *tx.tx_hash();
        match tx {
            TransactionSigned { transaction: Transaction::Legacy(tx), signature, .. } => {
                Ok(Self::Legacy(Signed::new_unchecked(tx, signature, hash)))
            }
            TransactionSigned { transaction: Transaction::Eip2930(tx), signature, .. } => {
                Ok(Self::Eip2930(Signed::new_unchecked(tx, signature, hash)))
            }
            TransactionSigned { transaction: Transaction::Eip1559(tx), signature, .. } => {
                Ok(Self::Eip1559(Signed::new_unchecked(tx, signature, hash)))
            }
            TransactionSigned { transaction: Transaction::Eip7702(tx), signature, .. } => {
                Ok(Self::Eip7702(Signed::new_unchecked(tx, signature, hash)))
            }
            // Not supported because missing blob sidecar
            TransactionSigned { transaction: Transaction::Eip4844(_), .. } => {
                Err(TransactionConversionError::UnsupportedForP2P)
            }
        }
    }
}

impl<T> From<Signed<T>> for TransactionSigned
where
    T: Into<Transaction>,
{
    fn from(value: Signed<T>) -> Self {
        let (tx, signature, hash) = value.into_parts();
        Self { transaction: tx.into(), signature, hash: hash.into() }
    }
}

impl From<PooledTransaction> for TransactionSigned {
    fn from(value: PooledTransaction) -> Self {
        match value {
            PooledTransaction::Legacy(tx) => tx.into(),
            PooledTransaction::Eip2930(tx) => tx.into(),
            PooledTransaction::Eip1559(tx) => tx.into(),
            PooledTransaction::Eip7702(tx) => tx.into(),
            PooledTransaction::Eip4844(tx) => {
                let (tx, signature, hash) = tx.into_parts();
                Signed::new_unchecked(tx.tx, signature, hash).into()
            }
        }
    }
}

/// Bincode-compatible transaction type serde implementations.
#[cfg(feature = "serde-bincode-compat")]
pub mod serde_bincode_compat {
    use alloc::borrow::Cow;
    use alloy_consensus::{
        transaction::serde_bincode_compat::{TxEip1559, TxEip2930, TxEip7702, TxLegacy},
        TxEip4844,
    };
    use alloy_primitives::{PrimitiveSignature as Signature, TxHash};
    use reth_primitives_traits::{serde_bincode_compat::SerdeBincodeCompat, SignedTransaction};
    use serde::{Deserialize, Serialize};

    /// Bincode-compatible [`super::Transaction`] serde implementation.
    #[derive(Debug, Serialize, Deserialize)]
    #[allow(missing_docs)]
    pub enum Transaction<'a> {
        Legacy(TxLegacy<'a>),
        Eip2930(TxEip2930<'a>),
        Eip1559(TxEip1559<'a>),
        Eip4844(Cow<'a, TxEip4844>),
        Eip7702(TxEip7702<'a>),
    }

    impl<'a> From<&'a super::Transaction> for Transaction<'a> {
        fn from(value: &'a super::Transaction) -> Self {
            match value {
                super::Transaction::Legacy(tx) => Self::Legacy(TxLegacy::from(tx)),
                super::Transaction::Eip2930(tx) => Self::Eip2930(TxEip2930::from(tx)),
                super::Transaction::Eip1559(tx) => Self::Eip1559(TxEip1559::from(tx)),
                super::Transaction::Eip4844(tx) => Self::Eip4844(Cow::Borrowed(tx)),
                super::Transaction::Eip7702(tx) => Self::Eip7702(TxEip7702::from(tx)),
            }
        }
    }

    impl<'a> From<Transaction<'a>> for super::Transaction {
        fn from(value: Transaction<'a>) -> Self {
            match value {
                Transaction::Legacy(tx) => Self::Legacy(tx.into()),
                Transaction::Eip2930(tx) => Self::Eip2930(tx.into()),
                Transaction::Eip1559(tx) => Self::Eip1559(tx.into()),
                Transaction::Eip4844(tx) => Self::Eip4844(tx.into_owned()),
                Transaction::Eip7702(tx) => Self::Eip7702(tx.into()),
            }
        }
    }

    /// Bincode-compatible [`super::TransactionSigned`] serde implementation.
    #[derive(Debug, Serialize, Deserialize)]
    pub struct TransactionSigned<'a> {
        hash: TxHash,
        signature: Signature,
        transaction: Transaction<'a>,
    }

    impl<'a> From<&'a super::TransactionSigned> for TransactionSigned<'a> {
        fn from(value: &'a super::TransactionSigned) -> Self {
            Self {
                hash: *value.tx_hash(),
                signature: value.signature,
                transaction: Transaction::from(&value.transaction),
            }
        }
    }

    impl<'a> From<TransactionSigned<'a>> for super::TransactionSigned {
        fn from(value: TransactionSigned<'a>) -> Self {
            Self {
                hash: value.hash.into(),
                signature: value.signature,
                transaction: value.transaction.into(),
            }
        }
    }
    impl SerdeBincodeCompat for super::TransactionSigned {
        type BincodeRepr<'a> = TransactionSigned<'a>;
    }

    #[cfg(test)]
    mod tests {
        use super::super::{serde_bincode_compat, Transaction, TransactionSigned};
        use arbitrary::Arbitrary;
        use rand::Rng;
        use reth_testing_utils::generators;
        use serde::{Deserialize, Serialize};

        #[test]
        fn test_transaction_bincode_roundtrip() {
            #[derive(Debug, Serialize, Deserialize)]
            struct Data<'a> {
                transaction: serde_bincode_compat::Transaction<'a>,
            }

            let mut bytes = [0u8; 1024];
            generators::rng().fill(bytes.as_mut_slice());
            let tx = Transaction::arbitrary(&mut arbitrary::Unstructured::new(&bytes)).unwrap();
            let data = Data { transaction: (&tx).into() };

            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data<'_> = bincode::deserialize(&encoded).unwrap();
            assert_eq!(tx, decoded.transaction.into());
        }

        #[test]
        fn test_transaction_signed_bincode_roundtrip() {
            #[derive(Debug, Serialize, Deserialize)]
            struct Data<'a> {
                transaction: serde_bincode_compat::TransactionSigned<'a>,
            }

            let mut bytes = [0u8; 1024];
            generators::rng().fill(bytes.as_mut_slice());
            let tx =
                TransactionSigned::arbitrary(&mut arbitrary::Unstructured::new(&bytes)).unwrap();
            let data = Data { transaction: (&tx).into() };

            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data<'_> = bincode::deserialize(&encoded).unwrap();
            assert_eq!(tx, decoded.transaction.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{
        constants::LEGACY_TX_TYPE_ID, Block, Transaction as _, TxEip1559, TxLegacy,
    };
    use alloy_eips::{
        eip2718::{Decodable2718, Encodable2718},
        eip7702::constants::SECP256K1N_HALF,
    };
    use alloy_primitives::{
        address, b256, bytes, hex, Address, Bytes, PrimitiveSignature as Signature, TxKind, B256,
        U256,
    };
    use alloy_rlp::{Decodable, Encodable, Error as RlpError};
    use reth_codecs::Compact;
    use reth_primitives_traits::SignedTransaction;
    use std::str::FromStr;

    #[test]
    fn eip_2_reject_high_s_value() {
        // This pre-homestead transaction has a high `s` value and should be rejected by the
        // `recover_signer` method:
        // https://etherscan.io/getRawTx?tx=0x9e6e19637bb625a8ff3d052b7c2fe57dc78c55a15d258d77c43d5a9c160b0384
        //
        // Block number: 46170
        let raw_tx = hex!("f86d8085746a52880082520894c93f2250589a6563f5359051c1ea25746549f0d889208686e75e903bc000801ba034b6fdc33ea520e8123cf5ac4a9ff476f639cab68980cd9366ccae7aef437ea0a0e517caa5f50e27ca0d1e9a92c503b4ccb039680c6d9d0c71203ed611ea4feb33");
        let tx = TransactionSigned::decode_2718(&mut &raw_tx[..]).unwrap();
        let signature = tx.signature();

        // make sure we know it's greater than SECP256K1N_HALF
        assert!(signature.s() > SECP256K1N_HALF);

        // recover signer, expect failure
        let hash = *tx.tx_hash();
        assert!(recover_signer(signature, hash).is_err());

        // use unchecked, ensure it succeeds (the signature is valid if not for EIP-2)
        assert!(recover_signer_unchecked(signature, hash).is_ok());
    }

    #[test]
    fn encode_decode_raw_block() {
        let bytes = hex!("f90288f90218a0fe21bb173f43067a9f90cfc59bbb6830a7a2929b5de4a61f372a9db28e87f9aea01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347940000000000000000000000000000000000000000a061effbbcca94f0d3e02e5bd22e986ad57142acabf0cb3d129a6ad8d0f8752e94a0d911c25e97e27898680d242b7780b6faef30995c355a2d5de92e6b9a7212ad3aa0056b23fbba480696b65fe5a59b8f2148a1299103c4f57df839233af2cf4ca2d2b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008003834c4b408252081e80a00000000000000000000000000000000000000000000000000000000000000000880000000000000000842806be9da056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421f869f86702842806be9e82520894658bdf435d810c91414ec09147daa6db624063798203e880820a95a040ce7918eeb045ebf8c8b1887ca139d076bda00fa828a07881d442a72626c42da0156576a68e456e295e4c9cf67cf9f53151f329438916e0f24fc69d6bbb7fbacfc0c0");
        let bytes_buf = &mut bytes.as_ref();
        let block = Block::<TransactionSigned>::decode(bytes_buf).unwrap();
        let mut encoded_buf = Vec::with_capacity(bytes.len());
        block.encode(&mut encoded_buf);
        assert_eq!(bytes[..], encoded_buf);
    }

    #[test]
    fn empty_block_rlp() {
        let body = alloy_consensus::BlockBody::<TransactionSigned>::default();
        let mut buf = Vec::new();
        body.encode(&mut buf);
        let decoded = alloy_consensus::BlockBody::decode(&mut buf.as_slice()).unwrap();
        assert_eq!(body, decoded);
    }

    #[test]
    fn test_decode_empty_typed_tx() {
        let input = [0x80u8];
        let res = TransactionSigned::decode(&mut &input[..]).unwrap_err();
        assert_eq!(RlpError::InputTooShort, res);
    }

    #[test]
    fn raw_kind_encoding_sanity() {
        // check the 0x80 encoding for Create
        let mut buf = Vec::new();
        TxKind::Create.encode(&mut buf);
        assert_eq!(buf, vec![0x80]);

        // check decoding
        let buf = [0x80];
        let decoded = TxKind::decode(&mut &buf[..]).unwrap();
        assert_eq!(decoded, TxKind::Create);
    }

    #[test]
    fn test_decode_create_goerli() {
        // test that an example create tx from goerli decodes properly
        let tx_bytes = hex!("b901f202f901ee05228459682f008459682f11830209bf8080b90195608060405234801561001057600080fd5b50610175806100206000396000f3fe608060405234801561001057600080fd5b506004361061002b5760003560e01c80630c49c36c14610030575b600080fd5b61003861004e565b604051610045919061011d565b60405180910390f35b60606020600052600f6020527f68656c6c6f2073746174656d696e64000000000000000000000000000000000060405260406000f35b600081519050919050565b600082825260208201905092915050565b60005b838110156100be5780820151818401526020810190506100a3565b838111156100cd576000848401525b50505050565b6000601f19601f8301169050919050565b60006100ef82610084565b6100f9818561008f565b93506101098185602086016100a0565b610112816100d3565b840191505092915050565b6000602082019050818103600083015261013781846100e4565b90509291505056fea264697066735822122051449585839a4ea5ac23cae4552ef8a96b64ff59d0668f76bfac3796b2bdbb3664736f6c63430008090033c080a0136ebffaa8fc8b9fda9124de9ccb0b1f64e90fbd44251b4c4ac2501e60b104f9a07eb2999eec6d185ef57e91ed099afb0a926c5b536f0155dd67e537c7476e1471");

        let decoded = TransactionSigned::decode(&mut &tx_bytes[..]).unwrap();
        assert_eq!(tx_bytes.len(), decoded.length());
        assert_eq!(tx_bytes, &alloy_rlp::encode(decoded)[..]);
    }

    #[test]
    fn test_decode_recover_mainnet_tx() {
        // random mainnet tx <https://etherscan.io/tx/0x86718885c4b4218c6af87d3d0b0d83e3cc465df2a05c048aa4db9f1a6f9de91f>
        let tx_bytes = hex!("02f872018307910d808507204d2cb1827d0094388c818ca8b9251b393131c08a736a67ccb19297880320d04823e2701c80c001a0cf024f4815304df2867a1a74e9d2707b6abda0337d2d54a4438d453f4160f190a07ac0e6b3bc9395b5b9c8b9e6d77204a236577a5b18467b9175c01de4faa208d9");

        let decoded = TransactionSigned::decode_2718(&mut &tx_bytes[..]).unwrap();
        assert_eq!(
            decoded.recover_signer().unwrap(),
            Address::from_str("0x95222290DD7278Aa3Ddd389Cc1E1d165CC4BAfe5").unwrap()
        );
    }

    #[test]
    // Test vector from https://sepolia.etherscan.io/tx/0x9a22ccb0029bc8b0ddd073be1a1d923b7ae2b2ea52100bae0db4424f9107e9c0
    // Blobscan: https://sepolia.blobscan.com/tx/0x9a22ccb0029bc8b0ddd073be1a1d923b7ae2b2ea52100bae0db4424f9107e9c0
    fn test_decode_recover_sepolia_4844_tx() {
        use alloy_primitives::{address, b256};

        // https://sepolia.etherscan.io/getRawTx?tx=0x9a22ccb0029bc8b0ddd073be1a1d923b7ae2b2ea52100bae0db4424f9107e9c0
        let raw_tx = alloy_primitives::hex::decode("0x03f9011d83aa36a7820fa28477359400852e90edd0008252089411e9ca82a3a762b4b5bd264d4173a242e7a770648080c08504a817c800f8a5a0012ec3d6f66766bedb002a190126b3549fce0047de0d4c25cffce0dc1c57921aa00152d8e24762ff22b1cfd9f8c0683786a7ca63ba49973818b3d1e9512cd2cec4a0013b98c6c83e066d5b14af2b85199e3d4fc7d1e778dd53130d180f5077e2d1c7a001148b495d6e859114e670ca54fb6e2657f0cbae5b08063605093a4b3dc9f8f1a0011ac212f13c5dff2b2c6b600a79635103d6f580a4221079951181b25c7e654901a0c8de4cced43169f9aa3d36506363b2d2c44f6c49fc1fd91ea114c86f3757077ea01e11fdd0d1934eda0492606ee0bb80a7bf8f35cc5f86ec60fe5031ba48bfd544").unwrap();
        let decoded = TransactionSigned::decode_2718(&mut raw_tx.as_slice()).unwrap();
        assert!(alloy_consensus::Typed2718::is_eip4844(&decoded));

        assert_eq!(
            decoded.recover_signer().ok(),
            Some(address!("A83C816D4f9b2783761a22BA6FADB0eB0606D7B2"))
        );

        let tx = decoded.transaction;

        assert_eq!(tx.to(), Some(address!("11E9CA82A3a762b4B5bd264d4173a242e7a77064")));

        assert_eq!(
            tx.blob_versioned_hashes(),
            Some(
                &[
                    b256!("012ec3d6f66766bedb002a190126b3549fce0047de0d4c25cffce0dc1c57921a"),
                    b256!("0152d8e24762ff22b1cfd9f8c0683786a7ca63ba49973818b3d1e9512cd2cec4"),
                    b256!("013b98c6c83e066d5b14af2b85199e3d4fc7d1e778dd53130d180f5077e2d1c7"),
                    b256!("01148b495d6e859114e670ca54fb6e2657f0cbae5b08063605093a4b3dc9f8f1"),
                    b256!("011ac212f13c5dff2b2c6b600a79635103d6f580a4221079951181b25c7e6549"),
                ][..]
            )
        );
    }

    #[test]
    fn decode_transaction_consumes_buffer() {
        let bytes = &mut &hex!("b87502f872041a8459682f008459682f0d8252089461815774383099e24810ab832a5b2a5425c154d58829a2241af62c000080c001a059e6b67f48fb32e7e570dfb11e042b5ad2e55e3ce3ce9cd989c7e06e07feeafda0016b83f4f980694ed2eee4d10667242b1f40dc406901b34125b008d334d47469")[..];
        let _transaction_res = TransactionSigned::decode(bytes).unwrap();
        assert_eq!(
            bytes.len(),
            0,
            "did not consume all bytes in the buffer, {:?} remaining",
            bytes.len()
        );
    }

    #[test]
    fn decode_multiple_network_txs() {
        let bytes = hex!("f86b02843b9aca00830186a094d3e8763675e4c425df46cc3b5c0f6cbdac39604687038d7ea4c68000802ba00eb96ca19e8a77102767a41fc85a36afd5c61ccb09911cec5d3e86e193d9c5aea03a456401896b1b6055311536bf00a718568c744d8c1f9df59879e8350220ca18");
        let transaction = Transaction::Legacy(TxLegacy {
            chain_id: Some(4u64),
            nonce: 2,
            gas_price: 1000000000,
            gas_limit: 100000,
            to: Address::from_str("d3e8763675e4c425df46cc3b5c0f6cbdac396046").unwrap().into(),
            value: U256::from(1000000000000000u64),
            input: Bytes::default(),
        });
        let signature = Signature::new(
            U256::from_str("0xeb96ca19e8a77102767a41fc85a36afd5c61ccb09911cec5d3e86e193d9c5ae")
                .unwrap(),
            U256::from_str("0x3a456401896b1b6055311536bf00a718568c744d8c1f9df59879e8350220ca18")
                .unwrap(),
            false,
        );
        let hash = b256!("a517b206d2223278f860ea017d3626cacad4f52ff51030dc9a96b432f17f8d34");
        test_decode_and_encode(&bytes, transaction, signature, Some(hash));

        let bytes = hex!("f86b01843b9aca00830186a094d3e8763675e4c425df46cc3b5c0f6cbdac3960468702769bb01b2a00802ba0e24d8bd32ad906d6f8b8d7741e08d1959df021698b19ee232feba15361587d0aa05406ad177223213df262cb66ccbb2f46bfdccfdfbbb5ffdda9e2c02d977631da");
        let transaction = Transaction::Legacy(TxLegacy {
            chain_id: Some(4),
            nonce: 1u64,
            gas_price: 1000000000,
            gas_limit: 100000,
            to: Address::from_slice(&hex!("d3e8763675e4c425df46cc3b5c0f6cbdac396046")[..]).into(),
            value: U256::from(693361000000000u64),
            input: Default::default(),
        });
        let signature = Signature::new(
            U256::from_str("0xe24d8bd32ad906d6f8b8d7741e08d1959df021698b19ee232feba15361587d0a")
                .unwrap(),
            U256::from_str("0x5406ad177223213df262cb66ccbb2f46bfdccfdfbbb5ffdda9e2c02d977631da")
                .unwrap(),
            false,
        );
        test_decode_and_encode(&bytes, transaction, signature, None);

        let bytes = hex!("f86b0384773594008398968094d3e8763675e4c425df46cc3b5c0f6cbdac39604687038d7ea4c68000802ba0ce6834447c0a4193c40382e6c57ae33b241379c5418caac9cdc18d786fd12071a03ca3ae86580e94550d7c071e3a02eadb5a77830947c9225165cf9100901bee88");
        let transaction = Transaction::Legacy(TxLegacy {
            chain_id: Some(4),
            nonce: 3,
            gas_price: 2000000000,
            gas_limit: 10000000,
            to: Address::from_slice(&hex!("d3e8763675e4c425df46cc3b5c0f6cbdac396046")[..]).into(),
            value: U256::from(1000000000000000u64),
            input: Bytes::default(),
        });
        let signature = Signature::new(
            U256::from_str("0xce6834447c0a4193c40382e6c57ae33b241379c5418caac9cdc18d786fd12071")
                .unwrap(),
            U256::from_str("0x3ca3ae86580e94550d7c071e3a02eadb5a77830947c9225165cf9100901bee88")
                .unwrap(),
            false,
        );
        test_decode_and_encode(&bytes, transaction, signature, None);

        let bytes = hex!("b87502f872041a8459682f008459682f0d8252089461815774383099e24810ab832a5b2a5425c154d58829a2241af62c000080c001a059e6b67f48fb32e7e570dfb11e042b5ad2e55e3ce3ce9cd989c7e06e07feeafda0016b83f4f980694ed2eee4d10667242b1f40dc406901b34125b008d334d47469");
        let transaction = Transaction::Eip1559(TxEip1559 {
            chain_id: 4,
            nonce: 26,
            max_priority_fee_per_gas: 1500000000,
            max_fee_per_gas: 1500000013,
            gas_limit: 21_000,
            to: Address::from_slice(&hex!("61815774383099e24810ab832a5b2a5425c154d5")[..]).into(),
            value: U256::from(3000000000000000000u64),
            input: Default::default(),
            access_list: Default::default(),
        });
        let signature = Signature::new(
            U256::from_str("0x59e6b67f48fb32e7e570dfb11e042b5ad2e55e3ce3ce9cd989c7e06e07feeafd")
                .unwrap(),
            U256::from_str("0x016b83f4f980694ed2eee4d10667242b1f40dc406901b34125b008d334d47469")
                .unwrap(),
            true,
        );
        test_decode_and_encode(&bytes, transaction, signature, None);

        let bytes = hex!("f8650f84832156008287fb94cf7f9e66af820a19257a2108375b180b0ec491678204d2802ca035b7bfeb9ad9ece2cbafaaf8e202e706b4cfaeb233f46198f00b44d4a566a981a0612638fb29427ca33b9a3be2a0a561beecfe0269655be160d35e72d366a6a860");
        let transaction = Transaction::Legacy(TxLegacy {
            chain_id: Some(4),
            nonce: 15,
            gas_price: 2200000000,
            gas_limit: 34811,
            to: Address::from_slice(&hex!("cf7f9e66af820a19257a2108375b180b0ec49167")[..]).into(),
            value: U256::from(1234),
            input: Bytes::default(),
        });
        let signature = Signature::new(
            U256::from_str("0x35b7bfeb9ad9ece2cbafaaf8e202e706b4cfaeb233f46198f00b44d4a566a981")
                .unwrap(),
            U256::from_str("0x612638fb29427ca33b9a3be2a0a561beecfe0269655be160d35e72d366a6a860")
                .unwrap(),
            true,
        );
        test_decode_and_encode(&bytes, transaction, signature, None);
    }

    fn test_decode_and_encode(
        bytes: &[u8],
        transaction: Transaction,
        signature: Signature,
        hash: Option<B256>,
    ) {
        let expected = TransactionSigned::new_unhashed(transaction, signature);
        if let Some(hash) = hash {
            assert_eq!(hash, *expected.tx_hash());
        }
        assert_eq!(bytes.len(), expected.length());

        let decoded = TransactionSigned::decode(&mut &bytes[..]).unwrap();
        assert_eq!(expected, decoded);
        assert_eq!(bytes, &alloy_rlp::encode(expected));
    }

    #[test]
    fn decode_raw_tx_and_recover_signer() {
        use alloy_primitives::hex_literal::hex;
        // transaction is from ropsten

        let hash: B256 =
            hex!("559fb34c4a7f115db26cbf8505389475caaab3df45f5c7a0faa4abfa3835306c").into();
        let signer: Address = hex!("641c5d790f862a58ec7abcfd644c0442e9c201b3").into();
        let raw = hex!("f88b8212b085028fa6ae00830f424094aad593da0c8116ef7d2d594dd6a63241bccfc26c80a48318b64b000000000000000000000000641c5d790f862a58ec7abcfd644c0442e9c201b32aa0a6ef9e170bca5ffb7ac05433b13b7043de667fbb0b4a5e45d3b54fb2d6efcc63a0037ec2c05c3d60c5f5f78244ce0a3859e3a18a36c61efb061b383507d3ce19d2");

        let mut pointer = raw.as_ref();
        let tx = TransactionSigned::decode(&mut pointer).unwrap();
        assert_eq!(*tx.tx_hash(), hash, "Expected same hash");
        let recovered = tx.recover_signer().expect("Recovering signer should pass");
        assert_eq!(recovered, signer);
    }

    #[test]
    fn test_envelop_encode() {
        // random tx: <https://etherscan.io/getRawTx?tx=0x9448608d36e721ef403c53b00546068a6474d6cbab6816c3926de449898e7bce>
        let input = hex!("02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76");
        let decoded = TransactionSigned::decode(&mut &input[..]).unwrap();

        let encoded = decoded.encoded_2718();
        assert_eq!(encoded[..], input);
    }

    #[test]
    fn test_envelop_decode() {
        // random tx: <https://etherscan.io/getRawTx?tx=0x9448608d36e721ef403c53b00546068a6474d6cbab6816c3926de449898e7bce>
        let input = bytes!("02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76");
        let decoded = TransactionSigned::decode_2718(&mut input.as_ref()).unwrap();

        let encoded = decoded.encoded_2718();
        assert_eq!(encoded, input);
    }

    #[test]
    fn test_decode_tx() {
        // some random transactions pulled from hive tests
        let data = hex!("b86f02f86c0705843b9aca008506fc23ac00830124f89400000000000000000000000000000000000003160180c001a00293c713e2f1eab91c366621ff2f867e05ad7e99d4aa5d069aafeb9e1e8c9b6aa05ec6c0605ff20b57c90a6484ec3b0509e5923733d06f9b69bee9a2dabe4f1352");
        let tx = TransactionSigned::decode(&mut data.as_slice()).unwrap();
        let mut b = Vec::with_capacity(data.len());
        tx.encode(&mut b);
        assert_eq!(data.as_slice(), b.as_slice());

        let data = hex!("f865048506fc23ac00830124f8940000000000000000000000000000000000000316018032a06b8fdfdcb84790816b7af85b19305f493665fe8b4e7c51ffdd7cc144cd776a60a028a09ab55def7b8d6602ba1c97a0ebbafe64ffc9c8e89520cec97a8edfb2ebe9");
        let tx = TransactionSigned::decode(&mut data.as_slice()).unwrap();
        let mut b = Vec::with_capacity(data.len());
        tx.encode(&mut b);
        assert_eq!(data.as_slice(), b.as_slice());
    }

    // <https://etherscan.io/tx/0x280cde7cdefe4b188750e76c888f13bd05ce9a4d7767730feefe8a0e50ca6fc4>
    #[test]
    fn recover_legacy_singer() {
        let data = hex!("f9015482078b8505d21dba0083022ef1947a250d5630b4cf539739df2c5dacb4c659f2488d880c46549a521b13d8b8e47ff36ab50000000000000000000000000000000000000000000066ab5a608bd00a23f2fe000000000000000000000000000000000000000000000000000000000000008000000000000000000000000048c04ed5691981c42154c6167398f95e8f38a7ff00000000000000000000000000000000000000000000000000000000632ceac70000000000000000000000000000000000000000000000000000000000000002000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc20000000000000000000000006c6ee5e31d828de241282b9606c8e98ea48526e225a0c9077369501641a92ef7399ff81c21639ed4fd8fc69cb793cfa1dbfab342e10aa0615facb2f1bcf3274a354cfe384a38d0cc008a11c2dd23a69111bc6930ba27a8");
        let tx = TransactionSigned::fallback_decode(&mut data.as_slice()).unwrap();
        assert_eq!(tx.ty(), LEGACY_TX_TYPE_ID);
        let sender = tx.recover_signer().unwrap();
        assert_eq!(sender, address!("a12e1462d0ceD572f396F58B6E2D03894cD7C8a4"));
    }

    // <https://github.com/alloy-rs/alloy/issues/141>
    // <https://etherscan.io/tx/0xce4dc6d7a7549a98ee3b071b67e970879ff51b5b95d1c340bacd80fa1e1aab31>
    #[test]
    fn recover_enveloped() {
        let data = hex!("02f86f0102843b9aca0085029e7822d68298f094d9e1459a7a482635700cbc20bbaf52d495ab9c9680841b55ba3ac080a0c199674fcb29f353693dd779c017823b954b3c69dffa3cd6b2a6ff7888798039a028ca912de909e7e6cdef9cdcaf24c54dd8c1032946dfa1d85c206b32a9064fe8");
        let tx = TransactionSigned::decode_2718(&mut data.as_slice()).unwrap();
        let sender = tx.recover_signer().unwrap();
        assert_eq!(sender, address!("001e2b7dE757bA469a57bF6b23d982458a07eFcE"));
        assert_eq!(tx.to(), Some(address!("D9e1459A7A482635700cBc20BBAF52D495Ab9C96")));
        assert_eq!(tx.input().as_ref(), hex!("1b55ba3a"));
        let encoded = tx.encoded_2718();
        assert_eq!(encoded.as_ref(), data.to_vec());
    }

    // <https://github.com/paradigmxyz/reth/issues/7750>
    // <https://etherscan.io/tx/0x2084b8144eea4031c2fa7dfe343498c5e665ca85ed17825f2925f0b5b01c36ac>
    #[test]
    fn recover_pre_eip2() {
        let data = hex!("f8ea0c850ba43b7400832dc6c0942935aa0a2d2fbb791622c29eb1c117b65b7a908580b884590528a9000000000000000000000001878ace42092b7f1ae1f28d16c1272b1aa80ca4670000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000d02ab486cedc0000000000000000000000000000000000000000000000000000557fe293cabc08cf1ca05bfaf3fda0a56b49cc78b22125feb5ae6a99d2b4781f00507d8b02c173771c85a0b5da0dbe6c5bc53740d0071fc83eb17ba0f709e49e9ae7df60dee625ef51afc5");
        let tx = TransactionSigned::decode_2718(&mut data.as_slice()).unwrap();
        let sender = tx.recover_signer();
        assert!(sender.is_err());
        let sender = tx.recover_signer_unchecked().unwrap();

        assert_eq!(sender, address!("7e9e359edf0dbacf96a9952fa63092d919b0842b"));
    }

    #[test]
    fn transaction_signed_no_hash_zstd_codec() {
        // will use same signature everywhere.
        // We don't need signature to match tx, just decoded to the same signature
        let signature = Signature::new(
            U256::from_str("0xeb96ca19e8a77102767a41fc85a36afd5c61ccb09911cec5d3e86e193d9c5ae")
                .unwrap(),
            U256::from_str("0x3a456401896b1b6055311536bf00a718568c744d8c1f9df59879e8350220ca18")
                .unwrap(),
            false,
        );

        let inputs: Vec<Vec<u8>> = vec![
            vec![],
            vec![0],
            vec![255],
            vec![1u8; 31],
            vec![255u8; 31],
            vec![1u8; 32],
            vec![255u8; 32],
            vec![1u8; 64],
            vec![255u8; 64],
        ];

        for input in inputs {
            let transaction = Transaction::Legacy(TxLegacy {
                chain_id: Some(4u64),
                nonce: 2,
                gas_price: 1000000000,
                gas_limit: 100000,
                to: Address::from_str("d3e8763675e4c425df46cc3b5c0f6cbdac396046").unwrap().into(),
                value: U256::from(1000000000000000u64),
                input: Bytes::from(input),
            });

            let tx = TransactionSigned::new_unhashed(transaction, signature);
            test_transaction_signed_to_from_compact(tx);
        }
    }

    fn test_transaction_signed_to_from_compact(tx: TransactionSigned) {
        // zstd aware `to_compact`
        let mut buff: Vec<u8> = Vec::new();
        let written_bytes = tx.to_compact(&mut buff);
        let (decoded, _) = TransactionSigned::from_compact(&buff, written_bytes);
        assert_eq!(tx, decoded);
    }

    #[test]
    fn create_txs_disallowed_for_eip4844() {
        let data =
            [3, 208, 128, 128, 123, 128, 120, 128, 129, 129, 128, 192, 129, 129, 192, 128, 128, 9];
        let res = TransactionSigned::decode_2718(&mut &data[..]);

        assert!(res.is_err());
    }
}
